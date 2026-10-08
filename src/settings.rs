use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
};

use crate::{
    config::Config,
    keys::Binding,
    prompt::EFFORTS,
    textinput::TextInput,
    theme::{self, THEMES, Theme},
    ui,
};

/// Values for claude --permission-mode; empty lets claude decide.
const MODES: &[&str] = &[
    "",
    "manual",
    "acceptEdits",
    "plan",
    "auto",
    "dontAsk",
    "bypassPermissions",
];

const TABS: &[&str] = &["General", "Sessions", "Keys", "Notifications", "Theme"];

#[derive(Debug, Clone, Copy, PartialEq)]
enum Field {
    DefaultModel,
    DefaultEffort,
    DefaultMode,
    Models,
    ProjectRoots,
    WorktreeDir,
    ClaudeCmd,
    IdleKill,
    Archive,
    Nag,
    Key(usize),
    NotifMac,
    NotifBell,
    NotifBadge,
    Theme,
}

const KEY_NAMES: &[&str] = &[
    "Switcher",
    "New prompt",
    "Diff viewer",
    "Settings",
    "Next session",
    "Previous session",
    "Focus agent list",
    "Focus pane",
    "Toggle graph",
    "Commit changes",
];

fn key_slot(cfg: &mut Config, i: usize) -> &mut String {
    let k = &mut cfg.keys;
    match i {
        0 => &mut k.switcher,
        1 => &mut k.new_prompt,
        2 => &mut k.diff,
        3 => &mut k.settings,
        4 => &mut k.next_session,
        5 => &mut k.prev_session,
        6 => &mut k.focus_list,
        7 => &mut k.focus_pane,
        8 => &mut k.graph,
        _ => &mut k.commit,
    }
}

fn fields(tab: usize) -> Vec<(Field, &'static str)> {
    match tab {
        0 => vec![
            (Field::DefaultModel, "Default model"),
            (Field::DefaultEffort, "Default effort"),
            (Field::DefaultMode, "Default permission mode"),
            (Field::Models, "Model list"),
            (Field::ProjectRoots, "Folder search roots"),
            (Field::WorktreeDir, "Worktree directory"),
            (Field::ClaudeCmd, "Claude command"),
        ],
        1 => vec![
            (Field::IdleKill, "Stop idle sessions after"),
            (Field::Archive, "Hide finished sessions after"),
            (Field::Nag, "Re-notify waiting sessions after"),
        ],
        2 => (0..KEY_NAMES.len())
            .map(|i| (Field::Key(i), KEY_NAMES[i]))
            .collect(),
        3 => vec![
            (Field::NotifMac, "macOS notifications"),
            (Field::NotifBell, "Terminal bell"),
            (Field::NotifBadge, "iTerm2 badge"),
        ],
        _ => vec![(Field::Theme, "Theme")],
    }
}

fn mins(n: u32) -> String {
    if n == 0 {
        "off".into()
    } else {
        format!("{n} min")
    }
}

fn value(cfg: &Config, f: Field) -> String {
    let on = |b: bool| {
        if b {
            "on".to_string()
        } else {
            "off".to_string()
        }
    };
    match f {
        Field::DefaultModel => cfg.default_model.clone(),
        Field::DefaultEffort => {
            if cfg.default_effort.is_empty() {
                "default".into()
            } else {
                cfg.default_effort.clone()
            }
        }
        Field::DefaultMode => {
            if cfg.default_mode.is_empty() {
                "default".into()
            } else {
                cfg.default_mode.clone()
            }
        }
        Field::Models => cfg.models.join(", "),
        Field::ProjectRoots => cfg.project_roots.join(", "),
        Field::WorktreeDir => cfg.worktree_dir.clone(),
        Field::ClaudeCmd => cfg.claude_command.clone(),
        Field::IdleKill => mins(cfg.timeouts.idle_kill),
        Field::Archive => mins(cfg.timeouts.archive_finished),
        Field::Nag => mins(cfg.timeouts.needs_input_nag),
        Field::Key(i) => key_slot(&mut cfg.clone(), i).clone(),
        Field::NotifMac => on(cfg.notifications.macos),
        Field::NotifBell => on(cfg.notifications.bell),
        Field::NotifBadge => on(cfg.notifications.badge),
        Field::Theme => theme::by_id(&cfg.theme).label.to_string(),
    }
}

fn edit_text(cfg: &Config, f: Field) -> String {
    match f {
        Field::IdleKill => cfg.timeouts.idle_kill.to_string(),
        Field::Archive => cfg.timeouts.archive_finished.to_string(),
        Field::Nag => cfg.timeouts.needs_input_nag.to_string(),
        _ => value(cfg, f),
    }
}

fn list(s: &str) -> Vec<String> {
    s.split(',')
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect()
}

pub enum SettingsAction {
    None,
    Close,
    Changed,
}

pub struct SettingsModal {
    tab: usize,
    sel: usize,
    editing: Option<TextInput>,
    capturing: bool,
    rect: Rect,
    tab_spans: Vec<(u16, u16)>,
}

impl SettingsModal {
    pub fn new() -> Self {
        Self {
            tab: 0,
            sel: 0,
            editing: None,
            capturing: false,
            rect: Rect::default(),
            tab_spans: vec![],
        }
    }

    pub fn paste(&mut self, s: &str) {
        if let Some(e) = &mut self.editing {
            e.insert_str(s);
        }
    }

    fn cycle(&self, cfg: &mut Config, f: Field, d: i32) -> bool {
        let step = |v: &mut u32| *v = (*v as i32 + d).max(0) as u32;
        match f {
            Field::DefaultModel => {
                let i = cfg
                    .models
                    .iter()
                    .position(|m| *m == cfg.default_model)
                    .map(|i| i as i32)
                    .unwrap_or(-1);
                let n = cfg.models.len() as i32;
                if n > 0 {
                    cfg.default_model = cfg.models[(i + d).rem_euclid(n) as usize].clone();
                }
            }
            Field::DefaultEffort => {
                let i = EFFORTS
                    .iter()
                    .position(|e| *e == cfg.default_effort)
                    .unwrap_or(0) as i32;
                cfg.default_effort =
                    EFFORTS[(i + d).rem_euclid(EFFORTS.len() as i32) as usize].into();
            }
            Field::DefaultMode => {
                let i = MODES
                    .iter()
                    .position(|m| *m == cfg.default_mode)
                    .unwrap_or(0) as i32;
                cfg.default_mode = MODES[(i + d).rem_euclid(MODES.len() as i32) as usize].into();
            }
            Field::IdleKill => step(&mut cfg.timeouts.idle_kill),
            Field::Archive => step(&mut cfg.timeouts.archive_finished),
            Field::Nag => step(&mut cfg.timeouts.needs_input_nag),
            Field::NotifMac => cfg.notifications.macos ^= true,
            Field::NotifBell => cfg.notifications.bell ^= true,
            Field::NotifBadge => cfg.notifications.badge ^= true,
            Field::Theme => {
                let i = THEMES.iter().position(|t| t.id == cfg.theme).unwrap_or(0) as i32;
                cfg.theme = THEMES[(i + d).rem_euclid(THEMES.len() as i32) as usize]
                    .id
                    .into();
            }
            _ => return false,
        }
        true
    }

    fn commit(&self, cfg: &mut Config, f: Field, text: &str) {
        let num = || text.trim().parse::<u32>().ok();
        match f {
            Field::DefaultModel => cfg.default_model = text.trim().into(),
            Field::Models => cfg.models = list(text),
            Field::ProjectRoots => cfg.project_roots = list(text),
            Field::WorktreeDir => cfg.worktree_dir = text.trim().into(),
            Field::ClaudeCmd => cfg.claude_command = text.trim().into(),
            Field::IdleKill => cfg.timeouts.idle_kill = num().unwrap_or(cfg.timeouts.idle_kill),
            Field::Archive => {
                cfg.timeouts.archive_finished = num().unwrap_or(cfg.timeouts.archive_finished)
            }
            Field::Nag => {
                cfg.timeouts.needs_input_nag = num().unwrap_or(cfg.timeouts.needs_input_nag)
            }
            Field::Key(i) => *key_slot(cfg, i) = text.trim().into(),
            _ => {}
        }
    }

    pub fn handle_key(&mut self, k: KeyEvent, cfg: &mut Config) -> SettingsAction {
        let fs = fields(self.tab);
        let field = fs.get(self.sel).map(|f| f.0);
        if self.capturing {
            self.capturing = false;
            if k.code != KeyCode::Esc
                && let Some(Field::Key(i)) = field
            {
                *key_slot(cfg, i) = Binding::from_event(&k).to_config();
                return SettingsAction::Changed;
            }
            return SettingsAction::None;
        }
        if let Some(e) = &mut self.editing {
            match k.code {
                KeyCode::Esc => self.editing = None,
                KeyCode::Enter => {
                    let text = e.text.clone();
                    self.editing = None;
                    if let Some(f) = field {
                        self.commit(cfg, f, &text);
                        return SettingsAction::Changed;
                    }
                }
                _ => {
                    e.handle_key(&k);
                }
            }
            return SettingsAction::None;
        }
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') => return SettingsAction::Close,
            KeyCode::Tab => self.set_tab((self.tab + 1) % TABS.len()),
            KeyCode::BackTab => self.set_tab((self.tab + TABS.len() - 1) % TABS.len()),
            KeyCode::Char(c @ '1'..='5') => self.set_tab(c as usize - '1' as usize),
            KeyCode::Down | KeyCode::Char('j') => {
                self.sel = (self.sel + 1).min(fs.len().saturating_sub(1))
            }
            KeyCode::Up | KeyCode::Char('k') => self.sel = self.sel.saturating_sub(1),
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Right | KeyCode::Char('l') => {
                let d = if matches!(k.code, KeyCode::Left | KeyCode::Char('h')) {
                    -1
                } else {
                    1
                };
                let d = if shift { d * 10 } else { d };
                if let Some(f) = field
                    && self.cycle(cfg, f, d)
                {
                    return SettingsAction::Changed;
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                if let Some(f) = field {
                    match f {
                        Field::Key(_) => self.capturing = true,
                        Field::DefaultEffort
                        | Field::DefaultMode
                        | Field::NotifMac
                        | Field::NotifBell
                        | Field::NotifBadge
                        | Field::Theme => {
                            self.cycle(cfg, f, 1);
                            return SettingsAction::Changed;
                        }
                        _ => self.editing = Some(TextInput::with_text(&edit_text(cfg, f), false)),
                    }
                }
            }
            KeyCode::Backspace | KeyCode::Delete => {
                if let Some(Field::Key(i)) = field {
                    key_slot(cfg, i).clear();
                    return SettingsAction::Changed;
                }
            }
            _ => {}
        }
        SettingsAction::None
    }

    fn set_tab(&mut self, t: usize) {
        self.tab = t;
        self.sel = 0;
    }

    pub fn handle_mouse(&mut self, m: MouseEvent, cfg: &mut Config) -> SettingsAction {
        if let MouseEventKind::Down(MouseButton::Left) = m.kind {
            let r = self.rect;
            if m.row == r.y + 1 {
                if let Some(i) = self
                    .tab_spans
                    .iter()
                    .position(|(a, b)| m.column >= *a && m.column < *b)
                {
                    self.set_tab(i);
                }
            } else if m.row >= r.y + 3 && m.row < r.y + r.height {
                let i = (m.row - r.y - 3) as usize;
                if i < fields(self.tab).len() {
                    if i == self.sel {
                        return self
                            .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), cfg);
                    }
                    self.sel = i;
                }
            }
        }
        SettingsAction::None
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, cfg: &Config, t: &Theme) {
        let w = 76.min(area.width);
        let h = 18.min(area.height);
        let r = Rect {
            x: area.x + (area.width - w) / 2,
            y: area.y + (area.height - h) / 3,
            width: w,
            height: h,
        };
        self.rect = r;
        f.render_widget(Clear, r);
        let hint = if self.capturing {
            " press the new key · esc cancel "
        } else if self.editing.is_some() {
            " enter save · esc cancel "
        } else {
            " tab section · j/k move · ←/→ change · enter edit · esc close "
        };
        let block = ui::modal_block(" Settings ", t)
            .title_bottom(Line::styled(hint, Style::default().fg(t.muted)));
        let inner = block.inner(r);
        f.render_widget(block, r);

        // Tab bar
        let mut spans = vec![Span::raw(" ")];
        self.tab_spans.clear();
        let mut x = inner.x + 1;
        for (i, name) in TABS.iter().enumerate() {
            let label = format!(" {name} ");
            let w = label.chars().count() as u16;
            self.tab_spans.push((x, x + w));
            x += w + 1;
            let st = if i == self.tab {
                Style::default()
                    .fg(t.bg)
                    .bg(t.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(t.muted)
            };
            spans.push(Span::styled(label, st));
            spans.push(Span::raw(" "));
        }
        let mut lines = vec![Line::from(spans), Line::raw("")];

        let label_w = 34usize;
        for (i, (field, label)) in fields(self.tab).into_iter().enumerate() {
            let selected = i == self.sel;
            let base = if selected {
                Style::default().bg(t.selection)
            } else {
                Style::default()
            };
            let v = if selected && self.capturing {
                "press a key…".to_string()
            } else if selected && self.editing.is_some() {
                self.editing.as_ref().unwrap().text.clone()
            } else {
                value(cfg, field)
            };
            let vstyle = if selected && (self.editing.is_some() || self.capturing) {
                base.fg(t.accent).add_modifier(Modifier::UNDERLINED)
            } else {
                base.fg(t.accent)
            };
            let arrows = matches!(
                field,
                Field::DefaultModel
                    | Field::DefaultEffort
                    | Field::DefaultMode
                    | Field::IdleKill
                    | Field::Archive
                    | Field::Nag
                    | Field::Theme
            );
            let mut l = Line::from(vec![
                Span::styled(format!("  {label:<label_w$}"), base.fg(t.fg)),
                Span::styled(
                    if arrows && selected {
                        format!("‹ {v} ›")
                    } else {
                        v
                    },
                    vstyle,
                ),
            ]);
            let used = l.width();
            l.spans.push(Span::styled(
                " ".repeat((inner.width as usize).saturating_sub(used)),
                base,
            ));
            lines.push(ui::truncate_line(l, inner.width as usize));
        }
        let note = match self.tab {
            1 => "Minutes; 0 turns a timeout off. Stopped sessions stay resumable.",
            2 => "These keys are captured even inside a claude pane. Backspace unbinds.",
            0 => "Lists are comma-separated. Model names are passed to claude --model.",
            _ => "",
        };
        if !note.is_empty() {
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                format!("  {note}"),
                Style::default().fg(t.muted),
            ));
        }
        f.render_widget(Paragraph::new(lines), inner);
        if let Some(e) = &self.editing {
            let y = inner.y + 2 + self.sel as u16;
            let x = inner.x + 2 + label_w as u16 + e.text[..e.cursor].chars().count() as u16;
            f.set_cursor_position((x.min(inner.x + inner.width - 1), y));
        }
    }
}
