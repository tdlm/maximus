use crossterm::event::{KeyCode, KeyEvent, MouseEvent, MouseEventKind};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
};
use unicode_width::UnicodeWidthStr;

use crate::{app::GlobalKeys, theme::Theme, ui};

/// A titled group of `(key, action)` rows.
type Section = (&'static str, Vec<(String, &'static str)>);

/// Every shortcut, grouped by where it works. Global keys follow the user's bindings.
fn sections(k: &GlobalKeys) -> Vec<Section> {
    let s = |v: &[(&str, &'static str)]| -> Vec<(String, &'static str)> {
        v.iter().map(|&(k, d)| (k.to_string(), d)).collect()
    };
    vec![
        (
            "Global",
            vec![
                (k.switcher.short(), "Switcher: sessions, projects, commands"),
                (k.new_prompt.short(), "New prompt"),
                (k.diff.short(), "Diff viewer for the current checkout"),
                (k.commit.short(), "Commit the current session's changes"),
                (k.terminal.short(), "Terminal in the current checkout"),
                (k.memory.short(), "Memory used by sessions and shells"),
                (k.overview.short(), "Overview of every agent"),
                (k.graph.short(), "Show/hide the commit graph"),
                (k.settings.short(), "Settings and key bindings"),
                (
                    format!("{} {}", k.next_session.short(), k.prev_session.short()),
                    "Next/previous session in the agent list",
                ),
                (
                    format!("{} {}", k.focus_list.short(), k.focus_pane.short()),
                    "Focus agent list / pane",
                ),
                (k.help.short(), "This list of shortcuts"),
            ],
        ),
        (
            "Agent list",
            s(&[
                ("j k", "Move"),
                ("⏎", "Open session (resumes a stopped one)"),
                ("n", "New prompt"),
                ("w", "New prompt in a new worktree"),
                ("N", "New project in a new folder"),
                ("o", "Overview of every agent"),
                ("r", "Resume session"),
                ("e", "Rename session"),
                ("x", "Close session / remove project"),
                ("m", "Merge the session's worktree"),
                ("d", "Diff viewer"),
                ("t", "Show/hide the commit graph"),
                ("s", "Sort: attention, name A→Z, Z→A"),
                ("/", "Switcher"),
                (",", "Settings"),
                ("?", "This list of shortcuts"),
                ("q", "Quit"),
            ]),
        ),
        (
            "Pane",
            s(&[
                ("⇧PgUp PgDn", "Scroll back (or the mouse wheel)"),
                ("drag", "Select and copy"),
            ]),
        ),
        (
            "Commit graph",
            s(&[
                ("j k", "Move through commits and files"),
                ("PgUp PgDn", "Scroll the selected file's diff"),
                ("h l", "Pan the diff"),
                ("s", "Split/unified diff"),
                ("y", "Copy the commit hash"),
                ("esc", "Collapse the commit, again to go back"),
                ("t", "Hide the graph"),
            ]),
        ),
        (
            "New prompt",
            s(&[
                ("⏎", "Send"),
                ("⌥⏎ ⇧⏎", "Newline"),
                ("^p", "Project"),
                ("^t", "Worktree: main, existing or new"),
                ("^o", "Model (← → effort)"),
                ("tab", "Rename the new branch"),
            ]),
        ),
        (
            "Diff viewer",
            s(&[
                ("j k", "Next/previous file"),
                ("J K", "Next/previous hunk"),
                ("⏎", "Focus the file"),
                ("] [", "Next/previous file"),
                ("s", "Split/unified"),
                ("h l", "Fold"),
                ("r", "Refresh"),
                ("esc", "Back / close"),
                (k.commit.short().as_str(), "Switch to commit mode"),
            ]),
        ),
        (
            "Commit",
            s(&[
                ("⏎", "Commit the checked files"),
                ("⌥⏎ ⇧⏎", "Newline"),
                ("tab", "Move to the file list"),
                ("space", "Check/uncheck a file or folder"),
                ("a", "Check/uncheck all"),
            ]),
        ),
        (
            "Terminal",
            vec![
                (k.terminal.short(), "Hide, leaving the shells running"),
                ("⌥t".into(), "Another shell in the same checkout"),
                (
                    format!("{} {}", k.prev_session.short(), k.next_session.short()),
                    "Previous/next shell",
                ),
                ("⌥1-9".into(), "Go to shell 1-9"),
                ("^d".into(), "End the shell"),
                ("⇧PgUp PgDn".into(), "Scroll back (or the mouse wheel)"),
            ],
        ),
        (
            "Overview",
            s(&[
                ("← ↑ ↓ →", "Move"),
                ("tab", "Next agent"),
                ("⏎", "Open the agent"),
                ("esc", "Close"),
            ]),
        ),
    ]
}

pub enum HelpAction {
    None,
    Close,
}

#[derive(Default)]
pub struct Help {
    /// First visible line.
    offset: usize,
    /// Lines and visible rows last drawn, for clamping scrolls.
    total: usize,
    visible: usize,
}

impl Help {
    fn max_offset(&self) -> usize {
        self.total.saturating_sub(self.visible)
    }

    fn scroll(&mut self, d: i64) {
        self.offset = (self.offset as i64 + d).clamp(0, self.max_offset() as i64) as usize;
    }

    pub fn handle_key(&mut self, k: KeyEvent) -> HelpAction {
        let page = self.visible.saturating_sub(1).max(1) as i64;
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => return HelpAction::Close,
            KeyCode::Down | KeyCode::Char('j') => self.scroll(1),
            KeyCode::Up | KeyCode::Char('k') => self.scroll(-1),
            KeyCode::PageDown | KeyCode::Char(' ') => self.scroll(page),
            KeyCode::PageUp => self.scroll(-page),
            KeyCode::Home | KeyCode::Char('g') => self.offset = 0,
            KeyCode::End | KeyCode::Char('G') => self.offset = self.max_offset(),
            _ => {}
        }
        HelpAction::None
    }

    pub fn handle_mouse(&mut self, m: MouseEvent) {
        match m.kind {
            MouseEventKind::ScrollDown => self.scroll(3),
            MouseEventKind::ScrollUp => self.scroll(-3),
            _ => {}
        }
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, keys: &GlobalKeys, t: &Theme) {
        let sections = sections(keys);
        let kw = sections
            .iter()
            .flat_map(|(_, rows)| rows.iter().map(|(k, _)| k.width()))
            .max()
            .unwrap_or(0);
        let key = Style::default().fg(t.accent).add_modifier(Modifier::BOLD);
        let muted = Style::default().fg(t.muted);
        let head = Style::default().fg(t.fg).add_modifier(Modifier::BOLD);

        let mut lines: Vec<Line<'static>> = vec![];
        for (i, (title, rows)) in sections.iter().enumerate() {
            if i > 0 {
                lines.push(Line::raw(""));
            }
            lines.push(Line::styled(format!(" {title}"), head));
            for (k, desc) in rows {
                lines.push(Line::from(vec![
                    Span::raw(" ".repeat(3 + kw - k.width())),
                    Span::styled(k.clone(), key),
                    Span::styled(format!("  {desc}"), muted),
                ]));
            }
        }
        let content_w = lines.iter().map(|l| l.width()).max().unwrap_or(0) as u16 + 2;

        let w = (content_w + 2).clamp(40.min(area.width), area.width);
        let h = (lines.len() as u16 + 2).min(area.height.saturating_sub(2).max(5));
        let r = Rect {
            x: area.x + (area.width - w) / 2,
            y: area.y + (area.height.saturating_sub(h)) / 2,
            width: w,
            height: h.min(area.height),
        };

        self.total = lines.len();
        self.visible = r.height.saturating_sub(2) as usize;
        self.offset = self.offset.min(self.max_offset());

        let mut title = " Keyboard shortcuts ".to_string();
        if self.total > self.visible {
            let last = (self.offset + self.visible).min(self.total);
            title = format!(
                " Keyboard shortcuts · {}–{last} of {} ",
                self.offset + 1,
                self.total
            );
        }
        f.render_widget(Clear, r);
        let block = ui::modal_block(&title, t).title_bottom(Line::from(vec![
            Span::styled(" ↑↓", key),
            Span::styled(" scroll  ", muted),
            Span::styled("esc", key),
            Span::styled(" / ", muted),
            Span::styled(keys.help.short(), key),
            Span::styled(" close ", muted),
        ]));
        let inner = block.inner(r);
        f.render_widget(block, r);
        let width = inner.width as usize;
        let lines: Vec<Line> = lines
            .into_iter()
            .skip(self.offset)
            .take(self.visible)
            .map(|l| ui::truncate_line(l, width))
            .collect();
        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    #[test]
    fn scrolls_within_the_list() {
        let mut h = Help {
            offset: 0,
            total: 30,
            visible: 10,
        };
        h.handle_key(key(KeyCode::Up));
        assert_eq!(h.offset, 0);
        h.handle_key(key(KeyCode::Char('j')));
        assert_eq!(h.offset, 1);
        h.handle_key(key(KeyCode::PageDown));
        assert_eq!(h.offset, 10);
        h.handle_key(key(KeyCode::End));
        assert_eq!(h.offset, 20);
        h.handle_key(key(KeyCode::Down));
        assert_eq!(h.offset, 20);
        h.handle_key(key(KeyCode::Home));
        assert_eq!(h.offset, 0);
        assert!(matches!(h.handle_key(key(KeyCode::Esc)), HelpAction::Close));
    }
}
