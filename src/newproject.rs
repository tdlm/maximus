use std::{
    fs,
    path::{Path, PathBuf},
};

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
};

use crate::{
    config::{expand, tilde},
    git,
    textinput::TextInput,
    theme::Theme,
    ui,
};

/// Where a new project would go, once the typed name checks out.
#[derive(Debug, PartialEq, Eq)]
pub struct Target {
    pub path: PathBuf,
    /// The highest missing folder above `path`, which gets created along with it.
    pub new_parent: Option<PathBuf>,
}

/// Resolves what was typed: a bare name or relative path lands under `base`; `~/…` or `/…`
/// goes exactly there.
pub fn resolve(input: &str, base: &Path) -> Result<Target, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Type a name for the folder".into());
    }
    let home = input == "~" || input.starts_with("~/");
    // Check the raw segments: `Path::components` quietly drops a `.` in the middle.
    for (i, seg) in input.split('/').enumerate() {
        match seg {
            "" => {}
            "~" if i == 0 && home => {}
            "." | ".." => return Err("“.” and “..” aren't allowed in the path".into()),
            name => check_name(name)?,
        }
    }
    let path = if home || input.starts_with('/') {
        expand(input)
    } else {
        base.join(input)
    };
    // Drop a trailing slash so the path reads the same as it's shown.
    let path: PathBuf = path.components().collect();

    if let Ok(meta) = fs::symlink_metadata(&path) {
        let what = if meta.is_dir() { "folder" } else { "file" };
        return Err(format!("A {what} already exists at {}", tilde(&path)));
    }
    let mut new_parent = None;
    let mut dir = path.parent();
    while let Some(d) = dir {
        match fs::metadata(d) {
            Ok(m) if m.is_dir() => break,
            Ok(_) => return Err(format!("{} is a file, not a folder", tilde(d))),
            Err(_) => new_parent = Some(d.to_path_buf()),
        }
        dir = d.parent();
    }
    Ok(Target { path, new_parent })
}

fn check_name(name: &str) -> Result<(), String> {
    if name.trim() != name {
        return Err(format!("“{name}” starts or ends with a space"));
    }
    if name.len() > 255 {
        return Err("Folder names can be at most 255 bytes".into());
    }
    if let Some(c) = name.chars().find(|c| c.is_control() || *c == ':') {
        let shown = if c == ':' {
            "“:”".to_string()
        } else {
            format!("{c:?}")
        };
        return Err(format!("Folder names can't contain {shown}"));
    }
    Ok(())
}

pub enum NewProjectAction {
    None,
    Close,
    /// The folder was made; git init's error, if it ran and failed.
    Created(PathBuf, Option<String>),
}

pub struct NewProject {
    input: TextInput,
    base: PathBuf,
    pub git: bool,
    /// Why the last attempt to create the folder failed; cleared on the next edit.
    failed: Option<String>,
}

impl NewProject {
    pub fn new(base: PathBuf, git: bool, text: &str) -> Self {
        Self {
            input: TextInput::with_text(text, false),
            base,
            git,
            failed: None,
        }
    }

    pub fn paste(&mut self, s: &str) {
        self.input.insert_str(s);
        self.failed = None;
    }

    pub fn handle_key(&mut self, k: KeyEvent) -> NewProjectAction {
        match k.code {
            KeyCode::Esc => return NewProjectAction::Close,
            KeyCode::Tab | KeyCode::BackTab => self.git = !self.git,
            KeyCode::Enter => {
                if let Ok(t) = resolve(&self.input.text, &self.base) {
                    return self.create(t);
                }
            }
            _ => {
                if self.input.handle_key(&k) {
                    self.failed = None;
                }
            }
        }
        NewProjectAction::None
    }

    fn create(&mut self, t: Target) -> NewProjectAction {
        let made = match t.path.parent() {
            Some(parent) => fs::create_dir_all(parent),
            None => Ok(()),
        }
        .and_then(|_| fs::create_dir(&t.path));
        if let Err(e) = made {
            self.failed = Some(format!("Couldn't create {}: {e}", tilde(&t.path)));
            return NewProjectAction::None;
        }
        let git_err = if self.git {
            git::init(&t.path).err().map(|e| format!("{e:#}"))
        } else {
            None
        };
        NewProjectAction::Created(t.path, git_err)
    }

    pub fn draw(&self, f: &mut Frame, area: Rect, t: &Theme) {
        let w = (area.width * 6 / 10).clamp(50.min(area.width), 90);
        let r = Rect {
            x: area.x + (area.width - w) / 2,
            y: area.y + area.height / 3,
            width: w,
            height: 8.min(area.height),
        };
        f.render_widget(Clear, r);
        let block = ui::modal_block(" New project ", t);
        let inner = block.inner(r);
        f.render_widget(block, r);

        let (lines, (_, ccol)) = self.input.layout(inner.width.saturating_sub(4) as usize);
        let text = lines.last().cloned().unwrap_or_default();
        let input = if self.input.text.is_empty() {
            Span::styled(
                "name, or a path like ~/elsewhere/app",
                Style::default().fg(t.muted),
            )
        } else {
            Span::styled(text, Style::default().fg(t.fg))
        };

        let status = match (&self.failed, resolve(&self.input.text, &self.base)) {
            (Some(e), _) => Line::styled(format!("   {e}"), Style::default().fg(t.red)),
            (None, _) if self.input.text.trim().is_empty() => Line::styled(
                format!("   in {}", tilde(&self.base)),
                Style::default().fg(t.muted),
            ),
            (None, Ok(target)) => {
                let mut spans = vec![
                    Span::styled("   → ", Style::default().fg(t.muted)),
                    Span::styled(tilde(&target.path), Style::default().fg(t.green)),
                ];
                if let Some(p) = target.new_parent {
                    spans.push(Span::styled(
                        format!("  (also creates {})", tilde(&p)),
                        Style::default().fg(t.yellow),
                    ));
                }
                Line::from(spans)
            }
            (None, Err(e)) => Line::styled(format!("   {e}"), Style::default().fg(t.red)),
        };

        let key = |s: &'static str| {
            Span::styled(
                s,
                Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
            )
        };
        let txt = |s: &'static str| Span::styled(s, Style::default().fg(t.muted));
        let check = if self.git { "[x]" } else { "[ ]" };
        f.render_widget(
            Paragraph::new(vec![
                Line::from(vec![
                    Span::styled(" › ", Style::default().fg(t.accent)),
                    input,
                ]),
                ui::truncate_line(status, inner.width as usize),
                Line::raw(""),
                Line::from(vec![
                    Span::styled(format!(" {check} "), Style::default().fg(t.accent)),
                    Span::styled("Initialize a git repository", Style::default().fg(t.fg)),
                ]),
                Line::raw(""),
                Line::from(vec![
                    key(" ⏎"),
                    txt(" create   "),
                    key("tab"),
                    txt(" toggle git   "),
                    key("esc"),
                    txt(" cancel"),
                ]),
            ]),
            inner,
        );
        f.set_cursor_position((inner.x + 3 + ccol as u16, inner.y));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("maximus-np-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn bare_names_land_in_the_base() {
        let b = base("bare");
        let t = resolve("  my-app ", &b).unwrap();
        assert_eq!(t.path, b.join("my-app"));
        assert_eq!(t.new_parent, None);
        let t = resolve("work/my-app/", &b).unwrap();
        assert_eq!(t.path, b.join("work/my-app"));
        assert_eq!(t.new_parent, Some(b.join("work")));
        fs::remove_dir_all(&b).unwrap();
    }

    #[test]
    fn absolute_paths_are_used_as_is() {
        let b = base("abs");
        let other = base("abs-other");
        let t = resolve(other.join("x").to_str().unwrap(), &b).unwrap();
        assert_eq!(t.path, other.join("x"));
        fs::remove_dir_all(&b).unwrap();
        fs::remove_dir_all(&other).unwrap();
    }

    #[test]
    fn rejects_bad_names_and_existing_paths() {
        let b = base("bad");
        fs::create_dir(b.join("taken")).unwrap();
        fs::write(b.join("file"), "").unwrap();
        for bad in [
            "",
            "   ",
            "../up",
            "a/./b",
            "a:b",
            "tab\there",
            "a/ b",
            "taken",
            "file",
            "file/x",
        ] {
            assert!(resolve(bad, &b).is_err(), "{bad:?} should be rejected");
        }
        assert!(resolve(&"x".repeat(256), &b).is_err());
        assert!(resolve(&"x".repeat(255), &b).is_ok());
        fs::remove_dir_all(&b).unwrap();
    }

    #[test]
    fn creates_the_folder_and_a_repo() {
        let b = base("create");
        let mut np = NewProject::new(b.clone(), true, "nested/app");
        let NewProjectAction::Created(p, err) = np.handle_key(KeyEvent::from(KeyCode::Enter))
        else {
            panic!("expected the folder to be created");
        };
        assert_eq!(p, b.join("nested/app"));
        assert_eq!(err, None);
        assert!(git::is_repo(&p));
        fs::remove_dir_all(&b).unwrap();
    }
}
