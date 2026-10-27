//! Keeping the source document out of the voters' reach.
//!
//! Voters are meant to read the repository. A judge asking "which of these two
//! would hurt more to defer" is far better informed for having seen the code,
//! the test coverage and the shape of the incident history — that is the whole
//! point of a voter over a prompt, and it is where the constraints of reality
//! enter the panel.
//!
//! What must not be in reach is the document being ranked. If the plan sits in
//! the tree the voter starts in, one `grep` collapses the panel into a
//! rehearsal of the ordering the plan already had, and nothing downstream can
//! detect that it happened: the ballots come back fluent and confident either
//! way.
//!
//! So the repository stays, and the plan leaves. The precheck is what makes
//! that a fact rather than an intention. `--isolate` is the fallback for when
//! the plan genuinely cannot be moved out.

use std::path::{Component, Path, PathBuf};

/// Absolute form of a path that need not exist yet (an `--out` directory is
/// checked before it is created). Lexical only: `..` is folded, nothing is read.
pub fn absolutize(path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    let mut out = PathBuf::new();
    for part in joined.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Absolutize, then resolve symlinks where the path exists. A symlink pointing
/// into the repo must not read as "outside" it.
pub fn resolve(path: &Path) -> PathBuf {
    let abs = absolutize(path);
    abs.canonicalize().unwrap_or_else(|_| {
        // The leaf may not exist; a resolved parent still fixes the tree.
        match (abs.parent(), abs.file_name()) {
            (Some(parent), Some(name)) => parent
                .canonicalize()
                .map(|p| p.join(name))
                .unwrap_or_else(|_| abs.clone()),
            _ => abs.clone(),
        }
    })
}

/// The tree a voter can walk from `cwd`: the enclosing git repo if there is
/// one, otherwise the directory itself. The repo is the right boundary because
/// a voter's tools are repo-scoped — `git grep`, `rg`, and an agent's own habit
/// of looking at the project root rather than only at `.`.
pub fn voter_root(cwd: &Path) -> PathBuf {
    let start = resolve(cwd);
    let mut dir = start.as_path();
    loop {
        if dir.join(".git").exists() {
            return dir.to_path_buf();
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => return start,
        }
    }
}

/// Can a voter starting in `root` reach `target` by ordinary exploration?
pub fn reachable(root: &Path, target: &Path) -> bool {
    resolve(target).starts_with(resolve(root))
}

/// An empty directory for voters to start in, so there is nothing to find.
pub fn isolated_cwd() -> std::io::Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("panel-voter-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("panel-iso-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    #[test]
    fn a_file_inside_the_tree_is_reachable() {
        let root = scratch("inside");
        let plan = root.join("docs/roadmap.md");
        std::fs::create_dir_all(plan.parent().unwrap()).unwrap();
        std::fs::write(&plan, "the plan").unwrap();
        assert!(reachable(&root, &plan));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_sibling_directory_is_not_reachable() {
        let base = scratch("sibling");
        let (repo, outside) = (base.join("repo"), base.join("elsewhere"));
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        assert!(!reachable(&repo, &outside.join("plan.md")));
        // A prefix match on the raw string would wrongly accept this one.
        assert!(!reachable(&repo, &base.join("repo-notes").join("plan.md")));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn dot_dot_cannot_be_used_to_look_outside() {
        let base = scratch("dotdot");
        let repo = base.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        // Lexically escapes the repo, so it must not read as contained.
        assert!(!reachable(&repo, &repo.join("../plan.md")));
        // ...and one that climbs out and back in must still read as contained.
        assert!(reachable(&repo, &repo.join("../repo/docs/plan.md")));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_pointing_into_the_tree_does_not_read_as_outside() {
        let base = scratch("symlink");
        let repo = base.join("repo");
        std::fs::create_dir_all(repo.join("docs")).unwrap();
        let real = repo.join("docs/plan.md");
        std::fs::write(&real, "the plan").unwrap();
        let link = base.join("plan-link.md");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(reachable(&repo, &link), "symlink must resolve to its target");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn voter_root_climbs_to_the_repo_not_just_the_directory() {
        let base = scratch("gitroot");
        let repo = base.join("repo");
        let deep = repo.join("a/b/c");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        assert_eq!(voter_root(&deep), repo.canonicalize().unwrap());
        // A file at the repo root is reachable from a deep subdirectory.
        assert!(reachable(&voter_root(&deep), &repo.join("ROADMAP.md")));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn without_a_repo_the_boundary_is_the_directory_itself() {
        let base = scratch("norepo");
        let deep = base.join("x/y");
        std::fs::create_dir_all(&deep).unwrap();
        assert_eq!(voter_root(&deep), deep.canonicalize().unwrap());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn an_isolated_cwd_is_empty() {
        let dir = isolated_cwd().unwrap();
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    }
}
