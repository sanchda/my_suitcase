//! Config file loading and target selection.
//!
//! A missing file is not fatal on its own (`--webhook` or `$BARK_WEBHOOK` can
//! supply the URL), but a file that is present and unparseable is, and unknown
//! keys are rejected so `webook =` fails loudly instead of posting nowhere.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Name given to the target declared by a top-level `webhook = "..."`.
pub const IMPLICIT: &str = "default";

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub webhook: String,
    #[serde(default)]
    pub username: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct File {
    /// Shorthand for a single destination; equivalent to `[targets.default]`.
    #[serde(default)]
    pub webhook: String,
    /// Target used when `--to` is omitted.
    #[serde(default)]
    pub default: Option<String>,
    /// Display name for targets that do not set their own.
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub targets: BTreeMap<String, Target>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub name: String,
    pub webhook: String,
    pub username: Option<String>,
}

/// `--config`, else `$BARK_CONFIG`, else `$XDG_CONFIG_HOME/bark/config.toml`,
/// else `~/.config/bark/config.toml`.
pub fn path(explicit: Option<PathBuf>, env: impl Fn(&str) -> Option<String>) -> PathBuf {
    if let Some(p) = explicit {
        return p;
    }
    if let Some(p) = env("BARK_CONFIG").filter(|s| !s.trim().is_empty()) {
        return PathBuf::from(p);
    }
    let base = env("XDG_CONFIG_HOME")
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env("HOME").unwrap_or_else(|| ".".to_string())).join(".config")
        });
    base.join("bark").join("config.toml")
}

pub fn load(path: &Path) -> Result<File, String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(File::default()),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    parse(&raw).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn parse(raw: &str) -> Result<File, String> {
    toml::from_str(raw).map_err(|e| e.message().to_string())
}

/// Every target name in the config, sorted, including the implicit one.
pub fn names(file: &File) -> Vec<String> {
    let mut out: Vec<String> = file.targets.keys().cloned().collect();
    if !file.webhook.trim().is_empty() && !file.targets.contains_key(IMPLICIT) {
        out.push(IMPLICIT.to_string());
    }
    out.sort();
    out
}

/// Pick a target. `want` is `--to`; `None` takes the config's default. A config
/// with exactly one target needs no `default =` line. `path` is only used to
/// point at the right file in error messages.
pub fn resolve(file: &File, want: Option<&str>, path: &Path) -> Result<Resolved, String> {
    let known = names(file);
    let at = path.display();

    let name = match want {
        Some(n) => n.to_string(),
        None => match file.default.as_deref().map(str::trim) {
            Some(d) if !d.is_empty() => d.to_string(),
            _ if !file.webhook.trim().is_empty() => IMPLICIT.to_string(),
            _ if known.len() == 1 => known[0].clone(),
            _ if known.is_empty() => {
                let why = if path.exists() {
                    "no webhook or [targets.*] entry"
                } else {
                    "file not found"
                };
                return Err(format!(
                    "no webhook configured: {at} ({why})\n       \
                     fix: bark init --webhook <url>, or set $BARK_WEBHOOK, or pass --webhook <url>"
                ));
            }
            _ => {
                return Err(format!(
                    "{at} has several targets ({}) and no `default =` line; pass --to <name>",
                    known.join(", ")
                ))
            }
        },
    };

    let target = if let Some(t) = file.targets.get(&name) {
        t.clone()
    } else if name == IMPLICIT && !file.webhook.trim().is_empty() {
        Target {
            webhook: file.webhook.clone(),
            username: None,
        }
    } else if known.is_empty() {
        return Err(format!("unknown target `{name}`: {at} defines none"));
    } else {
        return Err(format!(
            "unknown target `{name}` in {at} (known: {})",
            known.join(", ")
        ));
    };

    let webhook = target.webhook.trim().to_string();
    if webhook.is_empty() {
        return Err(format!("target `{name}` in {at} has an empty webhook"));
    }

    Ok(Resolved {
        name,
        username: target.username.or_else(|| file.username.clone()),
        webhook,
    })
}

/// Starter config written by `bark init`.
pub fn template(url: &str) -> String {
    format!(
        "# bark config -- see `bark --help`\n\
         # One destination; add `[targets.<name>]` blocks for more, then select\n\
         # them with `bark --to <name> ...`.\n\
         webhook = \"{url}\"\n\
         \n\
         # Display name for posts (optional; the webhook's own name is used otherwise).\n\
         # username = \"bark\"\n\
         \n\
         # [targets.alerts]\n\
         # webhook = \"https://discord.com/api/webhooks/<id>/<token>\"\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOWHERE: &str = "/nonexistent/bark/config.toml";

    fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.to_string())
        }
    }

    fn pick(file: &File, want: Option<&str>) -> Result<Resolved, String> {
        resolve(file, want, Path::new(NOWHERE))
    }

    #[test]
    fn path_precedence() {
        let env = env_of(&[
            ("BARK_CONFIG", "/etc/bark.toml"),
            ("XDG_CONFIG_HOME", "/xdg"),
            ("HOME", "/home/dave"),
        ]);
        assert_eq!(
            path(Some(PathBuf::from("/flag.toml")), &env),
            PathBuf::from("/flag.toml")
        );
        assert_eq!(path(None, &env), PathBuf::from("/etc/bark.toml"));

        let env = env_of(&[("XDG_CONFIG_HOME", "/xdg"), ("HOME", "/home/dave")]);
        assert_eq!(path(None, &env), PathBuf::from("/xdg/bark/config.toml"));

        let env = env_of(&[("HOME", "/home/dave")]);
        assert_eq!(
            path(None, &env),
            PathBuf::from("/home/dave/.config/bark/config.toml")
        );
    }

    #[test]
    fn empty_env_values_are_ignored() {
        let env = env_of(&[
            ("BARK_CONFIG", "  "),
            ("XDG_CONFIG_HOME", ""),
            ("HOME", "/home/dave"),
        ]);
        assert_eq!(
            path(None, &env),
            PathBuf::from("/home/dave/.config/bark/config.toml")
        );
    }

    #[test]
    fn missing_file_is_defaults() {
        assert_eq!(load(Path::new(NOWHERE)).unwrap(), File::default());
    }

    #[test]
    fn top_level_webhook_is_the_implicit_default() {
        let file = parse("webhook = \"https://example/hook\"").unwrap();
        let got = pick(&file, None).unwrap();
        assert_eq!(got.name, IMPLICIT);
        assert_eq!(got.webhook, "https://example/hook");
        assert_eq!(got.username, None);
    }

    #[test]
    fn sole_named_target_needs_no_default_line() {
        let file = parse("[targets.alerts]\nwebhook = \"https://example/a\"").unwrap();
        assert_eq!(pick(&file, None).unwrap().name, "alerts");
    }

    #[test]
    fn ambiguous_targets_require_a_choice() {
        let file = parse(
            "[targets.a]\nwebhook = \"https://example/a\"\n\
             [targets.b]\nwebhook = \"https://example/b\"\n",
        )
        .unwrap();
        let err = pick(&file, None).unwrap_err();
        assert!(err.contains("a, b") && err.contains(NOWHERE), "{err}");
        assert_eq!(pick(&file, Some("b")).unwrap().webhook, "https://example/b");
    }

    #[test]
    fn default_line_selects_among_targets() {
        let file = parse(
            "default = \"b\"\n\
             [targets.a]\nwebhook = \"https://example/a\"\n\
             [targets.b]\nwebhook = \"https://example/b\"\n",
        )
        .unwrap();
        assert_eq!(pick(&file, None).unwrap().name, "b");
    }

    #[test]
    fn per_target_username_beats_global() {
        let file = parse(
            "username = \"global\"\n\
             [targets.a]\nwebhook = \"https://example/a\"\n\
             [targets.b]\nwebhook = \"https://example/b\"\nusername = \"mine\"\n",
        )
        .unwrap();
        assert_eq!(
            pick(&file, Some("a")).unwrap().username.as_deref(),
            Some("global")
        );
        assert_eq!(
            pick(&file, Some("b")).unwrap().username.as_deref(),
            Some("mine")
        );
    }

    #[test]
    fn unknown_target_lists_known_ones() {
        let file = parse("[targets.alerts]\nwebhook = \"https://example/a\"").unwrap();
        let err = pick(&file, Some("nope")).unwrap_err();
        assert!(
            err.contains("known: alerts") && err.contains(NOWHERE),
            "{err}"
        );
    }

    #[test]
    fn empty_config_error_names_the_file_and_the_fix() {
        let err = pick(&File::default(), None).unwrap_err();
        assert!(err.contains(NOWHERE), "{err}");
        assert!(err.contains("file not found"), "{err}");
        assert!(err.contains("bark init --webhook"), "{err}");
        assert!(err.contains("$BARK_WEBHOOK"), "{err}");
    }

    #[test]
    fn empty_webhook_value_is_rejected() {
        let file = parse("[targets.a]\nwebhook = \"  \"").unwrap();
        assert!(pick(&file, Some("a"))
            .unwrap_err()
            .contains("empty webhook"));
    }

    #[test]
    fn typos_are_rejected() {
        assert!(parse("webook = \"https://example/hook\"").is_err());
        assert!(parse("[targets.a]\nwebhook = \"u\"\nusernam = \"x\"\n").is_err());
    }

    #[test]
    fn template_round_trips() {
        let file = parse(&template("https://example/hook")).unwrap();
        assert_eq!(pick(&file, None).unwrap().webhook, "https://example/hook");
    }
}
