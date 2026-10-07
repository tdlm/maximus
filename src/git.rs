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
