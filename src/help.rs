use crossterm::event::{KeyCode, KeyEvent, MouseEvent, MouseEventKind};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
};
use unicode_width::UnicodeWidthStr;

use crate::{app::GlobalKeys, textinput::TextInput, theme::Theme, ui};

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

/// Whether every word of the lowercased `query` appears in the row, its key spelled out
/// (`^g` also matches "ctrl g") or its section's title.
fn row_matches(query: &str, section: &str, key: &str, desc: &str) -> bool {
    let spelled = key
        .replace('^', "ctrl ")
        .replace('⌥', "alt ")
        .replace('⇧', "shift ");
    let hay = format!("{section} {key} {spelled} {desc}").to_lowercase();
    query.split_whitespace().all(|w| hay.contains(w))
}

/// Sections cut down to the rows matching `query`, dropping any left empty.
fn filter(sections: Vec<Section>, query: &str) -> Vec<Section> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return sections;
    }
    sections
        .into_iter()
        .filter_map(|(title, rows)| {
            let rows: Vec<_> = rows
                .into_iter()
                .filter(|(k, d)| row_matches(&query, title, k, d))
                .collect();
            (!rows.is_empty()).then_some((title, rows))
        })
        .collect()
}

#[derive(Default)]
pub struct Help {
    /// Filter typed at the top.
    input: TextInput,
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

    pub fn paste(&mut self, s: &str) {
        self.input.insert_str(s);
        self.offset = 0;
    }

    pub fn handle_key(&mut self, k: KeyEvent) -> HelpAction {
        let page = self.visible.saturating_sub(1).max(1) as i64;
        match k.code {
            KeyCode::Esc if !self.input.text.is_empty() => {
                self.input.set("");
                self.offset = 0;
            }
            KeyCode::Esc => return HelpAction::Close,
            KeyCode::Down => self.scroll(1),
            KeyCode::Up => self.scroll(-1),
            KeyCode::PageDown => self.scroll(page),
            KeyCode::PageUp => self.scroll(-page),
            KeyCode::Home => self.offset = 0,
            KeyCode::End => self.offset = self.max_offset(),
            _ => {
                let before = self.input.text.clone();
                self.input.handle_key(&k);
                if self.input.text != before {
                    self.offset = 0;
                }
            }
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
        let all = sections(keys);
        let key = Style::default().fg(t.accent).add_modifier(Modifier::BOLD);
        let muted = Style::default().fg(t.muted);
        let head = Style::default().fg(t.fg).add_modifier(Modifier::BOLD);
        let to_lines = |sections: &[Section], kw: usize| {
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
            lines
        };

        // Size the modal and key column from the full list so filtering doesn't resize it.
        let kw = all
            .iter()
            .flat_map(|(_, rows)| rows.iter().map(|(k, _)| k.width()))
            .max()
            .unwrap_or(0);
        let full = to_lines(&all, kw);
        let content_w = full.iter().map(|l| l.width()).max().unwrap_or(0) as u16 + 2;
        let w = (content_w + 2).clamp(40.min(area.width), area.width);
        // Borders, filter and separator around the list.
        let h = (full.len() as u16 + 4).min(area.height.saturating_sub(2).max(7));
        let r = Rect {
            x: area.x + (area.width - w) / 2,
            y: area.y + (area.height.saturating_sub(h)) / 2,
            width: w,
            height: h.min(area.height),
        };

        let lines = to_lines(&filter(all, &self.input.text), kw);
        self.total = lines.len();
        self.visible = r.height.saturating_sub(4) as usize;
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
        let esc = if self.input.text.is_empty() {
            " close "
        } else {
            " clear "
        };
        let block = ui::modal_block(&title, t).title_bottom(Line::from(vec![
            Span::styled(" ↑↓", key),
            Span::styled(" scroll  ", muted),
            Span::styled("esc", key),
            Span::styled(esc, muted),
        ]));
        let inner = block.inner(r);
        f.render_widget(block, r);

        let input_area = Rect { height: 1, ..inner };
        let (text, (_, ccol)) = self.input.layout(inner.width.saturating_sub(4) as usize);
        let text = text.last().cloned().unwrap_or_default();
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" › ", Style::default().fg(t.accent)),
                if self.input.text.is_empty() {
                    Span::styled("type to filter…", muted)
                } else {
                    Span::styled(text, Style::default().fg(t.fg))
                },
            ])),
            input_area,
        );
        f.set_cursor_position((input_area.x + 3 + ccol as u16, input_area.y));
        f.render_widget(
            Paragraph::new("─".repeat(inner.width as usize)).style(Style::default().fg(t.border)),
            Rect {
                y: inner.y + 1,
                height: 1,
                ..inner
            },
        );

        let list = Rect {
            y: inner.y + 2,
            height: inner.height.saturating_sub(2),
            ..inner
        };
        let width = list.width as usize;
        let lines: Vec<Line> = if lines.is_empty() {
            vec![Line::styled("  No matches", muted)]
        } else {
            lines
                .into_iter()
                .skip(self.offset)
                .take(self.visible)
                .map(|l| ui::truncate_line(l, width))
                .collect()
        };
        f.render_widget(Paragraph::new(lines), list);
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
            total: 30,
            visible: 10,
            ..Help::default()
        };
        h.handle_key(key(KeyCode::Up));
        assert_eq!(h.offset, 0);
        h.handle_key(key(KeyCode::Down));
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

    #[test]
    fn typing_filters_and_esc_clears_then_closes() {
        let mut h = Help {
            offset: 5,
            total: 30,
            visible: 10,
            ..Help::default()
        };
        for c in "j k".chars() {
            h.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(h.input.text, "j k");
        assert_eq!(h.offset, 0);
        assert!(matches!(h.handle_key(key(KeyCode::Esc)), HelpAction::None));
        assert_eq!(h.input.text, "");
        assert!(matches!(h.handle_key(key(KeyCode::Esc)), HelpAction::Close));
    }

    #[test]
    fn filters_rows_by_words_keys_and_section() {
        let k = GlobalKeys::from(&crate::config::Config::default());
        let rows = |q: &str| -> Vec<(&str, String)> {
            filter(sections(&k), q)
                .into_iter()
                .flat_map(|(t, rows)| rows.into_iter().map(move |(k, _)| (t, k)))
                .collect()
        };
        let all: usize = sections(&k).iter().map(|(_, r)| r.len()).sum();
        assert_eq!(rows(" ").len(), all);
        // Words match anywhere in the row, in any order.
        assert_eq!(
            rows("hunk previous"),
            vec![("Diff viewer", "J K".to_string())]
        );
        // Spelled-out modifiers match the short form.
        assert!(rows("ctrl g").contains(&("Global", "^g".to_string())));
        // A section title keeps all its rows.
        assert_eq!(
            rows("overview")
                .iter()
                .filter(|(t, _)| *t == "Overview")
                .count(),
            4
        );
        assert!(rows("zzz").is_empty());
    }
}
