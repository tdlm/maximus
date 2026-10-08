use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Result, bail};

fn git(dir: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output()?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(out.stdout)
}

fn git_str(dir: &Path, args: &[&str]) -> Result<String> {
    Ok(String::from_utf8_lossy(&git(dir, args)?).trim().to_string())
}

pub fn is_repo(dir: &Path) -> bool {
    git(dir, &["rev-parse", "--git-dir"]).is_ok()
}

pub fn toplevel(dir: &Path) -> Option<PathBuf> {
    git_str(dir, &["rev-parse", "--show-toplevel"])
        .ok()
        .map(PathBuf::from)
}

pub fn current_branch(dir: &Path) -> Option<String> {
    git_str(dir, &["rev-parse", "--abbrev-ref", "HEAD"])
        .ok()
        .filter(|s| !s.is_empty())
}

#[derive(Debug, Clone)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    pub main: bool,
}

pub fn worktrees(project: &Path) -> Vec<Worktree> {
    let Ok(out) = git_str(project, &["worktree", "list", "--porcelain"]) else {
        return vec![];
    };
    let mut res = Vec::new();
    for block in out.split("\n\n") {
        let mut path = None;
        let mut branch = String::from("(detached)");
        for line in block.lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                path = Some(PathBuf::from(p));
            } else if let Some(b) = line.strip_prefix("branch ") {
                branch = b.trim_start_matches("refs/heads/").to_string();
            }
        }
        if let Some(path) = path {
            res.push(Worktree {
                main: res.is_empty(),
                path,
                branch,
            });
        }
    }
    res
}

pub fn add_worktree(project: &Path, branch: &str, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let p = path.to_string_lossy();
    if git(project, &["worktree", "add", "-b", branch, &p]).is_err() {
        // Branch may already exist; check it out instead.
        git(project, &["worktree", "add", &p, branch])?;
    }
    Ok(())
}

/// The linked worktree checked out at `dir`, or None for the main checkout or a non-worktree.
pub fn linked_worktree(project: &Path, dir: &Path) -> Option<Worktree> {
    let dir = dir.canonicalize().ok()?;
    worktrees(project)
        .into_iter()
        .find(|w| !w.main && w.path.canonicalize().is_ok_and(|p| p == dir))
}

/// Merges a clean worktree's branch into the branch the main checkout is on, returning
/// that branch. A conflicted merge is aborted so the main checkout is left as it was.
pub fn merge_worktree(project: &Path, wt: &Worktree) -> Result<String> {
    if !changes(&wt.path)?.is_empty() {
        bail!("{} has uncommitted changes", wt.branch);
    }
    let base = current_branch(project)
        .filter(|b| b != "HEAD")
        .ok_or_else(|| anyhow::anyhow!("the main checkout is not on a branch"))?;
    if let Err(e) = git(project, &["merge", "--no-edit", &wt.branch]) {
        if git(project, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).is_ok() {
            let _ = git(project, &["merge", "--abort"]);
        }
        bail!("merging {} into {base} failed: {e}", wt.branch);
    }
    Ok(base)
}

/// Deletes a worktree and its branch. Without `force`, git refuses if the worktree has
/// changes or the branch isn't merged.
pub fn remove_worktree(project: &Path, wt: &Worktree, force: bool) -> Result<()> {
    let p = wt.path.to_string_lossy();
    if force {
        git(project, &["worktree", "remove", "--force", &p])?;
    } else {
        git(project, &["worktree", "remove", &p])?;
    }
    git(
        project,
        &["branch", if force { "-D" } else { "-d" }, &wt.branch],
    )?;
    Ok(())
}

/// Commits the working-tree state of `paths` (deletions and new files included), leaving
/// every other change out of the commit, even one already staged. Returns the new commit's
/// short hash.
pub fn commit_paths(dir: &Path, paths: &[String], message: &str) -> Result<String> {
    if paths.is_empty() {
        bail!("no files to commit");
    }
    let mut args = vec!["update-index", "--add", "--remove", "--"];
    args.extend(paths.iter().map(String::as_str));
    git(dir, &args)?;
    // --only commits just these paths from a temporary index; other staged changes stay staged.
    let mut args = vec![
        "--literal-pathspecs",
        "commit",
        "-q",
        "--only",
        "-m",
        message,
        "--",
    ];
    args.extend(paths.iter().map(String::as_str));
    git(dir, &args)?;
    git_str(dir, &["rev-parse", "--short", "HEAD"])
}

pub fn slug(s: &str, max_words: usize) -> String {
    let words: Vec<String> = s
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .take(max_words)
        .map(|w| w.to_ascii_lowercase())
        .collect();
    words.join("-")
}

#[derive(Debug, Clone)]
pub struct Change {
    pub path: String,
    pub old_path: Option<String>,
    /// One of M A D R ?
    pub status: char,
}

pub fn changes(dir: &Path) -> Result<Vec<Change>> {
    let out = git(
        dir,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    let mut parts = out.split(|b| *b == 0).filter(|p| !p.is_empty());
    let mut res = Vec::new();
    while let Some(entry) = parts.next() {
        if entry.len() < 4 {
            continue;
        }
        let (x, y) = (entry[0] as char, entry[1] as char);
        let path = String::from_utf8_lossy(&entry[3..]).to_string();
        let mut old_path = None;
        let status = if x == '?' {
            '?'
        } else if x == 'R' || x == 'C' {
            old_path = parts.next().map(|p| String::from_utf8_lossy(p).to_string());
            'R'
        } else if x == 'D' || y == 'D' {
            'D'
        } else if x == 'A' {
            'A'
        } else {
            'M'
        };
        res.push(Change {
            path,
            old_path,
            status,
        });
    }
    res.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(res)
}

/// File contents at HEAD, or None if absent.
pub fn head_contents(dir: &Path, path: &str) -> Option<Vec<u8>> {
    git(dir, &["show", &format!("HEAD:{path}")]).ok()
}

#[derive(Debug, Clone)]
pub struct Commit {
    pub hash: String,
    /// Unix seconds.
    pub time: u64,
    /// Branch and tag names pointing here, as `git log %D` prints them.
    pub refs: String,
    pub subject: String,
}

/// One line of `git log --graph`: the graph drawing, plus the commit when the line has one.
#[derive(Debug, Clone)]
pub struct GraphRow {
    pub graph: String,
    pub commit: Option<Commit>,
}

/// The most recent `n` commits reachable from the checkout's HEAD, newest first.
pub fn graph(dir: &Path, n: usize) -> Result<Vec<GraphRow>> {
    let out = git(
        dir,
        &[
            "log",
            "--graph",
            "--color=never",
            &format!("-{n}"),
            "--format=%x1f%h%x1f%ct%x1f%D%x1f%s",
        ],
    )?;
    Ok(String::from_utf8_lossy(&out)
        .lines()
        .map(|l| {
            let mut p = l.split('\x1f');
            let graph = p.next().unwrap_or("").trim_end().to_string();
            let commit = (|| {
                Some(Commit {
                    hash: p.next()?.to_string(),
                    time: p.next()?.parse().ok()?,
                    refs: p.next()?.to_string(),
                    subject: p.collect::<Vec<_>>().join("\x1f"),
                })
            })();
            GraphRow { graph, commit }
        })
        .collect())
}

/// Files a commit changed relative to its first parent (or everything, for a root commit).
pub fn commit_files(dir: &Path, hash: &str) -> Result<Vec<Change>> {
    let parent = format!("{hash}^");
    let out = git(
        dir,
        &[
            "diff-tree",
            "-r",
            "-M",
            "--name-status",
            "-z",
            &parent,
            hash,
        ],
    )
    .or_else(|_| {
        git(
            dir,
            &["diff-tree", "-r", "--root", "--name-status", "-z", hash],
        )
    })?;
    let mut parts = out
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).to_string());
    let mut res = Vec::new();
    // With --root, diff-tree prints the commit hash first.
    while let Some(code) = parts.next() {
        let Some(status) = code.chars().next().filter(|_| code.len() <= 4) else {
            continue;
        };
        let Some(first) = parts.next() else { break };
        let (path, old_path, status) = match status {
            'R' | 'C' => match parts.next() {
                Some(new) => (new, Some(first), 'R'),
                None => break,
            },
            'A' | 'D' => (first, None, status),
            _ => (first, None, 'M'),
        };
        res.push(Change {
            path,
            old_path,
            status,
        });
    }
    Ok(res)
}

/// File contents at `rev`, or None if absent.
pub fn show_file(dir: &Path, rev: &str, path: &str) -> Option<Vec<u8>> {
    git(dir, &["show", &format!("{rev}:{path}")]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("maximus-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let main = root.join("main");
        std::fs::create_dir_all(&main).unwrap();
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["config", "user.name", "t"],
            &["config", "user.email", "t@t"],
            &["commit", "-q", "--allow-empty", "-m", "init"],
        ] {
            git(&main, args).unwrap();
        }
        main
    }

    fn commit_file(dir: &Path, file: &str) {
        std::fs::write(dir.join(file), file).unwrap();
        git(dir, &["add", file]).unwrap();
        git(dir, &["commit", "-q", "-m", file]).unwrap();
    }

    #[test]
    fn merges_and_removes_a_worktree() {
        let main = repo("merge");
        let path = main.with_file_name("feat");
        add_worktree(&main, "feat", &path).unwrap();
        let wt = linked_worktree(&main, &path).unwrap();
        assert!(linked_worktree(&main, &main).is_none());

        std::fs::write(path.join("dirty"), "").unwrap();
        assert!(merge_worktree(&main, &wt).is_err());
        std::fs::remove_file(path.join("dirty")).unwrap();

        commit_file(&path, "a");
        assert_eq!(merge_worktree(&main, &wt).unwrap(), "main");
        assert!(main.join("a").exists());
        remove_worktree(&main, &wt, false).unwrap();
        assert!(!path.exists());
        assert!(git(&main, &["rev-parse", "--verify", "feat"]).is_err());
        let _ = std::fs::remove_dir_all(main.parent().unwrap());
    }

    #[test]
    fn commits_only_the_chosen_paths() {
        let main = repo("commit");
        for f in ["edit", "gone", "staged-gone", "old", "kept"] {
            commit_file(&main, f);
        }
        std::fs::write(main.join("edit"), "changed").unwrap();
        std::fs::remove_file(main.join("gone")).unwrap();
        git(&main, &["rm", "-q", "staged-gone"]).unwrap();
        git(&main, &["mv", "old", "new"]).unwrap();
        std::fs::write(main.join("fresh *"), "").unwrap();
        // Staged but left out of the commit.
        std::fs::write(main.join("kept"), "changed").unwrap();
        git(&main, &["add", "kept"]).unwrap();

        let paths: Vec<String> = ["edit", "gone", "staged-gone", "new", "old", "fresh *"]
            .map(String::from)
            .into();
        let hash = commit_paths(&main, &paths, "pick some").unwrap();
        assert!(!hash.is_empty());
        let left: Vec<(String, char)> = changes(&main)
            .unwrap()
            .into_iter()
            .map(|c| (c.path, c.status))
            .collect();
        assert_eq!(left, [("kept".to_string(), 'M')]);
        // Still staged.
        assert_eq!(
            git_str(&main, &["diff", "--cached", "--name-only"]).unwrap(),
            "kept"
        );
        assert!(commit_paths(&main, &[], "nothing").is_err());
        let _ = std::fs::remove_dir_all(main.parent().unwrap());
    }

    #[test]
    fn aborts_a_conflicted_merge() {
        let main = repo("conflict");
        let path = main.with_file_name("feat");
        add_worktree(&main, "feat", &path).unwrap();
        let wt = linked_worktree(&main, &path).unwrap();
        std::fs::write(path.join("a"), "theirs").unwrap();
        git(&path, &["add", "a"]).unwrap();
        git(&path, &["commit", "-q", "-m", "theirs"]).unwrap();
        commit_file(&main, "a");

        assert!(merge_worktree(&main, &wt).is_err());
        assert!(git(&main, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).is_err());
        assert!(changes(&main).unwrap().is_empty());

        remove_worktree(&main, &wt, true).unwrap();
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(main.parent().unwrap());
    }
}
