//! `<dir>/inbox/` — the queue every CLI backlog mutation goes through.
//!
//! Enqueue and drain are separate on purpose: a writer only ever creates its own
//! uniquely named request file, so writers never contend and no lock is needed.
//! Applying the queue is what is serialized, by the `drain.pid` guard, and the
//! loop owns `BACKLOG.md` outright for the length of an iteration.

use crate::backlog_edit;
use crate::pidguard;
use crate::R;
use serde::{Deserialize, Serialize};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// A queued mutation, carrying everything needed to replay it at drain time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Request {
    Add {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        under: Option<String>,
        title: String,
        body: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    Edit {
        id: String,
        title: String,
        verify: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    Drop {
        id: String,
        recursive: bool,
    },
    Done {
        id: String,
    },
    Uncheck {
        id: String,
    },
}

/// The result of replaying one request against a backlog text.
pub struct Applied {
    pub text: String,
    /// One line for the caller, `run.log` and the webhook.
    pub summary: String,
    /// A dropped subtree, held aside for the archive.
    pub archive: Option<String>,
}

impl Request {
    /// Replay against `current`. The same call lints at enqueue time and applies
    /// at drain time, so a request that lints clean fails later only if the
    /// backlog moved under it.
    pub fn apply(&self, current: &str) -> Result<Applied, String> {
        let plain = |text: String, summary: String| Applied {
            text,
            summary,
            archive: None,
        };
        match self {
            Request::Add {
                id,
                under,
                title,
                body,
                model,
            } => {
                let (text, new_id) = match (id, under) {
                    (Some(id), _) => backlog_edit::apply_add_with_id(current, id, title, body)?,
                    (None, Some(parent)) => {
                        backlog_edit::apply_add_under(current, parent, title, body)?
                    }
                    (None, None) => backlog_edit::apply_add_top(current, title, body)?,
                };
                let text = match model {
                    Some(model) => backlog_edit::apply_model(&text, &new_id, model)?,
                    None => text,
                };
                Ok(plain(text, format!("added task {new_id}")))
            }
            Request::Edit {
                id,
                title,
                verify,
                model,
            } => {
                let text = backlog_edit::apply_edit(current, id, title, verify)?;
                let text = match model {
                    Some(model) => backlog_edit::apply_model(&text, id, model)?,
                    None => text,
                };
                Ok(plain(text, format!("edited task {id}")))
            }
            Request::Drop { id, recursive } => {
                let (text, removed) = backlog_edit::apply_drop(current, id, *recursive)?;
                Ok(Applied {
                    text,
                    summary: format!("dropped task {id} ({} lines)", removed.lines().count()),
                    archive: Some(removed),
                })
            }
            Request::Done { id } => {
                let text = backlog_edit::apply_done(current, id)?;
                Ok(plain(text, format!("checked off task {id}")))
            }
            Request::Uncheck { id } => {
                let text = backlog_edit::apply_uncheck(current, id)?;
                Ok(plain(text, format!("reopened task {id}")))
            }
        }
    }
}

/// What a drain did, for the caller to log and notify on.
#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    pub applied: Vec<String>,
    pub rejected: Vec<String>,
}

impl Outcome {
    /// Only the tests care whether a drain was a no-op; the loop logs both lists.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.applied.is_empty() && self.rejected.is_empty()
    }
}

pub fn dir(base: &Path) -> PathBuf {
    base.join("inbox")
}

/// Where an unapplyable request is parked — evidence, not garbage.
pub fn rejected_dir(base: &Path) -> PathBuf {
    dir(base).join("rejected")
}

/// The guard that serializes drainers; same `create_new` + liveness probe as the
/// loop's own pidfile.
pub fn drain_pidfile(base: &Path) -> PathBuf {
    base.join("drain.pid")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Queue `req`. The filename carries the ordering (timestamp, then pid and a
/// per-process counter to break ties), so nothing has to be arbitrated.
pub fn enqueue(base: &Path, req: &Request) -> R<PathBuf> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let queue = dir(base);
    std::fs::create_dir_all(&queue)?;
    let body = serde_json::to_string(req)?;
    let ts = now();
    for _ in 0..64 {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let stem = format!("{ts:010}-{:06}-{seq:04}", std::process::id());
        let path = queue.join(format!("{stem}.json"));
        if path.exists() {
            continue;
        }
        // Write under a name the drainer ignores, then rename it in: a request
        // must never be visible half-written, or a drain would reject it.
        let tmp = queue.join(format!(".{stem}.tmp"));
        std::fs::write(&tmp, &body)?;
        std::fs::rename(&tmp, &path)?;
        return Ok(path);
    }
    Err("inbox: no free request filename".into())
}

/// Queued requests, oldest first — filenames sort by timestamp.
fn pending(base: &Path) -> R<Vec<PathBuf>> {
    let queue = dir(base);
    if !queue.exists() {
        return Ok(Vec::new());
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(&queue)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    Ok(files)
}

pub fn has_pending(base: &Path) -> bool {
    pending(base).map(|p| !p.is_empty()).unwrap_or(true)
}

pub fn has_done(base: &Path, id: &str) -> bool {
    pending(base).unwrap_or_default().iter().any(|p| {
        std::fs::read_to_string(p)
            .ok()
            .and_then(|s| serde_json::from_str::<Request>(&s).ok())
            .is_some_and(|r| matches!(r, Request::Done { id: q } if q == id))
    })
}

/// Park a request that cannot be applied. Removing it is the fallback only
/// because a file that will not move re-rejects on every future iteration.
fn reject(base: &Path, path: &Path) {
    let dest = rejected_dir(base);
    let name = path.file_name().unwrap_or_default();
    if std::fs::create_dir_all(&dest).is_err() || std::fs::rename(path, dest.join(name)).is_err() {
        let _ = std::fs::remove_file(path);
    }
}

/// Append a dropped subtree to `<dir>/archive/dropped-<ts>.md`, so `drop` is
/// never destructive.
fn archive_drop(base: &Path, blocks: &[String]) -> R<()> {
    let dest = base.join("archive");
    std::fs::create_dir_all(&dest)?;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dest.join(format!("dropped-{}.md", now())))?;
    for block in blocks {
        writeln!(f, "{block}")?;
    }
    Ok(())
}

/// Drop any queued check-off of `id`, returning how many were discarded.
///
/// The agent closes its own leaf with `ralph done <id>`, which — like every CLI
/// mutation — only queues while the loop runs. So at judge time the leaf is still
/// unchecked on disk and the judge's `apply_uncheck` is a no-op, and the queued
/// check-off then lands at the next drain and silently re-closes the very leaf the
/// judge reopened. Without this the refutation is cosmetic: routing never
/// re-selects the leaf and the loop moves on as if the judge had passed it.
pub fn discard_done(base: &Path, id: &str) -> R<usize> {
    if pending(base)?.is_empty() {
        return Ok(0);
    }
    // Same guard as `drain`, so this can never delete a file mid-replay.
    let Ok(_guard) = pidguard::acquire(&drain_pidfile(base)) else {
        return Ok(0);
    };
    let mut discarded = 0;
    for path in pending(base)? {
        let queued = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Request>(&raw).ok());
        if matches!(queued, Some(Request::Done { id: ref q }) if q == id)
            && std::fs::remove_file(&path).is_ok()
        {
            discarded += 1;
        }
    }
    Ok(discarded)
}

/// Apply the whole queue in order, under the drain guard. A busy guard yields an
/// empty outcome: the holder is applying the same requests, and draining is
/// idempotent over the queue either way.
pub fn drain(base: &Path, backlog: &Path) -> R<Outcome> {
    let files = pending(base)?;
    if files.is_empty() {
        return Ok(Outcome::default());
    }
    let Ok(_guard) = pidguard::acquire(&drain_pidfile(base)) else {
        return Ok(Outcome::default());
    };
    // Re-list under the guard; the previous holder may have just applied these.
    let files = pending(base)?;
    let mut text = match std::fs::read_to_string(backlog) {
        Ok(t) => t,
        // An absent backlog is the post-completion state: `add` bootstraps it.
        Err(e) if e.kind() == ErrorKind::NotFound => backlog_edit::empty_backlog(),
        Err(e) => return Err(format!("{}: cannot read backlog: {e}", backlog.display()).into()),
    };
    let mut outcome = Outcome::default();
    let mut done = Vec::new();
    let mut archives = Vec::new();
    for path in files {
        let replayed = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|raw| serde_json::from_str::<Request>(&raw).map_err(|e| e.to_string()))
            .and_then(|req| req.apply(&text));
        match replayed {
            Ok(applied) => {
                text = applied.text;
                archives.extend(applied.archive);
                outcome.applied.push(applied.summary);
                done.push(path);
            }
            Err(e) => {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                outcome.rejected.push(format!("{name}: {e}"));
                reject(base, &path);
            }
        }
    }
    if !done.is_empty() {
        // Archive first: a failed backlog write is retried, and a duplicated
        // archive entry is harmless where a lost subtree is not.
        if !archives.is_empty() {
            archive_drop(base, &archives)?;
        }
        backlog_edit::write_atomic(backlog, &text)?;
        // Only now are the requests safe to forget.
        for path in done {
            let _ = std::fs::remove_file(path);
        }
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backlog::{Document, SCHEMA_MARKER};
    use std::sync::atomic::AtomicUsize;

    fn tmp() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let base = std::env::temp_dir().join(format!(
            "ralph-inbox-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    fn seeded(base: &Path) -> PathBuf {
        let backlog = base.join("BACKLOG.md");
        std::fs::write(
            &backlog,
            format!("{SCHEMA_MARKER}\n# B\n- [ ] **1 — First.** Verify: y\n"),
        )
        .unwrap();
        backlog
    }

    fn add(title: &str) -> Request {
        Request::Add {
            id: None,
            under: None,
            title: title.into(),
            body: "Verify: cargo test".into(),
            model: None,
        }
    }

    #[test]
    fn enqueue_does_not_touch_the_backlog() {
        let base = tmp();
        let backlog = seeded(&base);
        let before = std::fs::read_to_string(&backlog).unwrap();
        enqueue(&base, &add("Queued")).unwrap();
        assert_eq!(std::fs::read_to_string(&backlog).unwrap(), before);
        assert_eq!(pending(&base).unwrap().len(), 1);
    }

    #[test]
    fn drain_applies_in_filename_order_and_clears_the_queue() {
        let base = tmp();
        let backlog = seeded(&base);
        enqueue(&base, &add("Second")).unwrap();
        enqueue(&base, &add("Third")).unwrap();

        let outcome = drain(&base, &backlog).unwrap();
        assert_eq!(outcome.applied, vec!["added task 2", "added task 3"]);
        assert!(outcome.rejected.is_empty());
        assert!(pending(&base).unwrap().is_empty());

        let doc = Document::parse(&std::fs::read_to_string(&backlog).unwrap());
        assert!(!doc.has_errors(), "{:?}", doc.issues);
        assert_eq!(doc.tasks[1].title, "Second");
        assert_eq!(doc.tasks[2].title, "Third");
    }

    #[test]
    fn drain_is_idempotent_over_an_empty_queue() {
        let base = tmp();
        let backlog = seeded(&base);
        enqueue(&base, &add("Once")).unwrap();
        assert_eq!(drain(&base, &backlog).unwrap().applied.len(), 1);
        assert!(drain(&base, &backlog).unwrap().is_empty());
        assert_eq!(
            Document::parse(&std::fs::read_to_string(&backlog).unwrap())
                .tasks
                .len(),
            2
        );
    }

    #[test]
    fn a_held_guard_defers_the_drain_without_losing_requests() {
        let base = tmp();
        let backlog = seeded(&base);
        enqueue(&base, &add("Later")).unwrap();
        let held = pidguard::acquire(&drain_pidfile(&base)).unwrap();

        assert!(drain(&base, &backlog).unwrap().is_empty());
        assert_eq!(pending(&base).unwrap().len(), 1);

        drop(held);
        assert_eq!(drain(&base, &backlog).unwrap().applied.len(), 1);
    }

    #[test]
    fn a_request_invalid_at_drain_time_is_parked_not_deleted() {
        let base = tmp();
        let backlog = seeded(&base);
        enqueue(&base, &Request::Done { id: "nope".into() }).unwrap();
        enqueue(&base, &add("Fine")).unwrap();
        std::fs::write(dir(&base).join("0000000000-000000-9999.json"), "{ not json").unwrap();

        let outcome = drain(&base, &backlog).unwrap();
        assert_eq!(outcome.applied, vec!["added task 2"]);
        assert_eq!(outcome.rejected.len(), 2);
        assert!(outcome.rejected.iter().any(|r| r.contains("no task")));
        assert_eq!(std::fs::read_dir(rejected_dir(&base)).unwrap().count(), 2);
        assert!(pending(&base).unwrap().is_empty());
    }

    #[test]
    fn drop_archives_the_subtree_it_removes() {
        let base = tmp();
        let backlog = base.join("BACKLOG.md");
        std::fs::write(
            &backlog,
            format!(
                "{SCHEMA_MARKER}\n# B\n- [ ] **1 — Keep.** Verify: y\n- [ ] **2 — Go.**\n  Verify: y\n  - [ ] **2.1 — Child.** Verify: y\n"
            ),
        )
        .unwrap();
        enqueue(
            &base,
            &Request::Drop {
                id: "2".into(),
                recursive: true,
            },
        )
        .unwrap();

        let outcome = drain(&base, &backlog).unwrap();
        assert_eq!(outcome.rejected, Vec::<String>::new());
        let text = std::fs::read_to_string(&backlog).unwrap();
        assert!(!text.contains("2.1"), "{text}");

        let archived: String = std::fs::read_dir(base.join("archive"))
            .unwrap()
            .map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap())
            .collect();
        assert!(archived.contains("**2 — Go.**"), "{archived}");
        assert!(archived.contains("**2.1 — Child.**"), "{archived}");
    }

    #[test]
    fn add_bootstraps_a_backlog_that_completion_archived_away() {
        let base = tmp();
        let backlog = base.join("BACKLOG.md");
        enqueue(&base, &add("First of new arc")).unwrap();
        assert_eq!(
            drain(&base, &backlog).unwrap().applied,
            vec!["added task 1"]
        );
        let doc = Document::parse(&std::fs::read_to_string(&backlog).unwrap());
        assert!(!doc.has_errors(), "{:?}", doc.issues);
    }

    // The bug this guards: `ralph done <id>` only queues while the loop runs, so at judge
    // time the leaf is still unchecked on disk. A refutation that does not also drop the
    // queued check-off is undone by the very next drain.
    #[test]
    fn discard_done_drops_only_the_matching_check_off() {
        let tmp = std::env::temp_dir().join(format!("ralph-discard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(dir(&tmp)).unwrap();

        enqueue(&tmp, &Request::Done { id: "2.2".into() }).unwrap();
        enqueue(&tmp, &Request::Done { id: "3".into() }).unwrap();
        enqueue(&tmp, &Request::Uncheck { id: "2.2".into() }).unwrap();
        enqueue(&tmp, &add("keep me")).unwrap();

        assert_eq!(discard_done(&tmp, "2.2").unwrap(), 1);
        // Only 2.2's check-off goes; another leaf's, and other request kinds, survive.
        assert_eq!(discard_done(&tmp, "2.2").unwrap(), 0);

        let left: Vec<Request> = pending(&tmp)
            .unwrap()
            .iter()
            .map(|p| serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap())
            .collect();
        assert_eq!(left.len(), 3);
        assert!(left.contains(&Request::Done { id: "3".into() }));
        assert!(left.contains(&Request::Uncheck { id: "2.2".into() }));
        assert!(!left.contains(&Request::Done { id: "2.2".into() }));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn old_requests_without_model_still_replay_and_edits_preserve_strictness() {
        let add: Request =
            serde_json::from_str(r#"{"kind":"add","title":"Task","body":"Verify: true"}"#).unwrap();
        let added = add.apply(&backlog_edit::empty_backlog()).unwrap();
        assert!(added.text.contains("**1 — Task**\n"));
        let strict = backlog_edit::apply_model(&added.text, "1", "!opus").unwrap();
        let edit: Request =
            serde_json::from_str(r#"{"kind":"edit","id":"1","title":"Renamed","verify":"true"}"#)
                .unwrap();
        assert!(edit
            .apply(&strict)
            .unwrap()
            .text
            .contains("**1 — Renamed** !opus —"));
    }

    #[test]
    fn requests_round_trip_through_json() {
        for req in [
            add("T"),
            Request::Add {
                id: Some("3.1".into()),
                under: None,
                title: "T".into(),
                body: "Verify: y".into(),
                model: Some("!opus".into()),
            },
            Request::Edit {
                id: "1".into(),
                title: "T".into(),
                verify: "y".into(),
                model: Some("sonnet".into()),
            },
            Request::Drop {
                id: "1".into(),
                recursive: true,
            },
            Request::Done { id: "1".into() },
            Request::Uncheck { id: "1".into() },
        ] {
            let raw = serde_json::to_string(&req).unwrap();
            assert_eq!(serde_json::from_str::<Request>(&raw).unwrap(), req);
        }
    }
}
