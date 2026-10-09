use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
};

use crate::{config::expand, textinput::TextInput, theme::Theme, ui};

#[derive(Debug, Clone)]
pub enum Cmd {
    NewPrompt,
    NewWorktree,
    /// Opens the new-project modal with this text filled in.
    NewProject(String),
    Diff,
    Settings,
    RemoveProject(PathBuf),
    CloseSession(String),
    RenameSession(String),
    ResumeSession(String),
    Commit,
    Terminal,
    Memory,
    MergeWorktree(String),
    DiscardWorktree(String),
    DefaultModel(String),
    Theme(&'static str),
    Quit,
}

#[derive(Debug, Clone)]
pub enum Action {
    Session(String),
    Project(PathBuf),
    AddFolder(PathBuf),
    Cmd(Cmd),
}

#[derive(Debug, Clone)]
pub struct Item {
    pub label: String,
    pub detail: String,
    pub tag: &'static str,
    pub dot: Option<(String, Color)>,
    pub action: Action,
}

pub struct Switcher {
    input: TextInput,
    items: Vec<Item>,
    /// Folders offered for "Add folder" (only shown once you type).
    folders: Vec<Item>,
    shown: Vec<Item>,
    sel: usize,
    offset: usize,
    list_rect: Rect,
    matcher: SkimMatcherV2,
}

impl Switcher {
    pub fn new(items: Vec<Item>, folders: Vec<Item>) -> Self {
        let mut s = Self {
            input: TextInput::new(false),
            items,
            folders,
            shown: vec![],
            sel: 0,
            offset: 0,
            list_rect: Rect::default(),
            matcher: SkimMatcherV2::default().ignore_case(),
        };
        s.refilter();
        s
    }

    fn refilter(&mut self) {
        let q = self.input.text.trim().to_string();
        self.sel = 0;
        self.offset = 0;
        if q.is_empty() {
            self.shown = self.items.clone();
            return;
        }
        let mut scored: Vec<(i64, usize, Item)> = self
            .items
            .iter()
            .chain(self.folders.iter())
            .enumerate()
            .filter_map(|(i, it)| {
                let hay = format!("{} {} {}", it.label, it.detail, it.tag);
                self.matcher
                    .fuzzy_match(&hay, &q)
                    .map(|s| (s, i, it.clone()))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        self.shown = scored.into_iter().map(|(_, _, it)| it).collect();
        // A literal path that isn't listed yet: add it, or create it if it's not there.
        if q.starts_with('/') || q.starts_with('~') {
            let p = expand(&q);
            let shown = crate::config::tilde(&p);
            let item = if p.is_dir() {
                Item {
                    label: format!("Add folder {shown}"),
                    detail: String::new(),
                    tag: "add",
                    dot: None,
                    action: Action::AddFolder(p),
                }
            } else {
                Item {
                    label: format!("New project at {shown}"),
                    detail: String::new(),
                    tag: "create",
                    dot: None,
                    action: Action::Cmd(Cmd::NewProject(q.clone())),
                }
            };
            self.shown.insert(0, item);
        }
    }

    pub fn paste(&mut self, s: &str) {
        self.input.insert_str(s);
        self.refilter();
    }

    /// Returns Some(action) when an item is chosen; Err(()) to close.
    pub fn handle_key(&mut self, k: KeyEvent) -> Result<Option<Action>, ()> {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Esc => return Err(()),
            KeyCode::Enter => return Ok(self.shown.get(self.sel).map(|i| i.action.clone())),
            KeyCode::Down | KeyCode::Tab => self.move_sel(1),
            KeyCode::Char('n') if ctrl => self.move_sel(1),
            KeyCode::Up | KeyCode::BackTab => self.move_sel(-1),
            KeyCode::Char('p') if ctrl => self.move_sel(-1),
            KeyCode::PageDown => self.move_sel(10),
            KeyCode::PageUp => self.move_sel(-10),
            _ => {
                let before = self.input.text.clone();
                self.input.handle_key(&k);
                if self.input.text != before {
                    self.refilter();
                }
            }
        }
        Ok(None)
    }

    fn move_sel(&mut self, d: i32) {
        if self.shown.is_empty() {
            return;
        }
        let n = self.shown.len() as i32;
        self.sel = (self.sel as i32 + d).clamp(0, n - 1) as usize;
    }

    pub fn handle_mouse(&mut self, m: MouseEvent) -> Option<Action> {
        let r = self.list_rect;
        match m.kind {
            MouseEventKind::Down(MouseButton::Left)
                if m.column >= r.x
                    && m.column < r.x + r.width
                    && m.row >= r.y
                    && m.row < r.y + r.height =>
            {
                let i = self.offset + (m.row - r.y) as usize;
                if i < self.shown.len() {
                    if i == self.sel {
                        return Some(self.shown[i].action.clone());
                    }
                    self.sel = i;
                }
            }
            MouseEventKind::ScrollDown => self.move_sel(1),
            MouseEventKind::ScrollUp => self.move_sel(-1),
            _ => {}
        }
        None
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let w = (area.width * 7 / 10).clamp(40.min(area.width), 100);
        let h = (area.height * 6 / 10).clamp(8.min(area.height), 30);
        let r = Rect {
            x: area.x + (area.width - w) / 2,
            y: area.y + area.height / 6,
            width: w,
            height: h,
        };
        f.render_widget(Clear, r);
        let block = ui::modal_block(" Go to ", t);
        let inner = block.inner(r);
        f.render_widget(block, r);

        let input_area = Rect { height: 1, ..inner };
        let (lines, (_, ccol)) = self.input.layout(inner.width.saturating_sub(4) as usize);
        let text = lines.last().cloned().unwrap_or_default();
        let placeholder = self.input.text.is_empty();
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" › ", Style::default().fg(t.accent)),
                if placeholder {
                    Span::styled(
                        "sessions, projects, commands, folders…",
                        Style::default().fg(t.muted),
                    )
                } else {
                    Span::styled(text, Style::default().fg(t.fg))
                },
            ])),
            input_area,
        );
        f.set_cursor_position((input_area.x + 3 + ccol as u16, input_area.y));
        let sep = Rect {
            y: inner.y + 1,
            height: 1,
            ..inner
        };
        f.render_widget(
            Paragraph::new("─".repeat(inner.width as usize)).style(Style::default().fg(t.border)),
            sep,
        );

        self.list_rect = Rect {
            y: inner.y + 2,
            height: inner.height.saturating_sub(2),
            ..inner
        };
        let lh = self.list_rect.height as usize;
        if self.sel < self.offset {
            self.offset = self.sel;
        } else if lh > 0 && self.sel >= self.offset + lh {
            self.offset = self.sel + 1 - lh;
        }
        let width = inner.width as usize;
        let mut out = Vec::new();
        if self.shown.is_empty() {
            out.push(Line::styled("  No matches", Style::default().fg(t.muted)));
        }
        for (i, it) in self.shown.iter().enumerate().skip(self.offset).take(lh) {
            let selected = i == self.sel;
            let base = if selected {
                Style::default().bg(t.selection)
            } else {
                Style::default()
            };
            let mut spans = vec![Span::styled(" ", base)];
            match &it.dot {
                Some((d, c)) => spans.push(Span::styled(format!("{d} "), base.fg(*c))),
                None => spans.push(Span::styled("  ", base)),
            }
            let label_style = if selected {
                base.fg(t.fg).add_modifier(Modifier::BOLD)
            } else {
                base.fg(t.fg)
            };
            spans.push(Span::styled(it.label.clone(), label_style));
            if !it.detail.is_empty() {
                spans.push(Span::styled(format!("  {}", it.detail), base.fg(t.muted)));
            }
            let used: usize = spans.iter().map(|s| s.width()).sum();
            let tag = format!("{} ", it.tag);
            if used + tag.len() < width {
                spans.push(Span::styled(" ".repeat(width - used - tag.len()), base));
                spans.push(Span::styled(tag, base.fg(t.muted)));
            } else {
                let mut line = Line::from(spans);
                line = ui::truncate_line(line, width);
                out.push(line.style(base));
                continue;
            }
            out.push(Line::from(spans));
        }
        f.render_widget(Paragraph::new(out), self.list_rect);
    }
}
