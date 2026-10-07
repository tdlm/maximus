use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use crossbeam_channel::Sender;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;

use crate::{
    config::{Config, ProjectEntry, State, expand, tilde},
    diff::{DiffAction, DiffView},
    git,
    hooks::HookMsg,
    keys::{self, KeySet},
    notify,
    prompt::{Launch, Prompt, PromptAction, Tree},
    session::{self, Session, Status},
    settings::{SettingsAction, SettingsModal},
    switcher::{Action, Cmd, Item, Switcher},
    theme::{self, THEMES, Theme},
    ui,
};

pub enum AppEvent {
    Input(Event),
    Output,
    Exited(String, u64),
    Hook(HookMsg),
    Folders(Vec<PathBuf>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum RowKey {
    Project(PathBuf),
    Session(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Focus {
    List,
    Pane,
}

pub enum ConfirmAction {
    Quit,
    RemoveProject(PathBuf),
    CloseSession(String),
}

pub struct Confirm {
    pub message: String,
    pub action: ConfirmAction,
}

// Only one modal exists at a time, so variant size differences don't matter.
#[allow(clippy::large_enum_variant)]
pub enum Modal {
    Switcher(Box<Switcher>),
    Prompt(Prompt),
    Diff(Box<DiffView>),
    Settings(SettingsModal),
    Confirm(Confirm),
}

pub struct GlobalKeys {
    pub switcher: KeySet,
    pub new_prompt: KeySet,
    pub diff: KeySet,
    pub settings: KeySet,
    pub next_session: KeySet,
    pub prev_session: KeySet,
    pub focus_list: KeySet,
    pub focus_pane: KeySet,
}

impl GlobalKeys {
    fn from(cfg: &Config) -> Self {
        let k = &cfg.keys;
        Self {
            switcher: KeySet::parse(&k.switcher),
            new_prompt: KeySet::parse(&k.new_prompt),
            diff: KeySet::parse(&k.diff),
            settings: KeySet::parse(&k.settings),
            next_session: KeySet::parse(&k.next_session),
            prev_session: KeySet::parse(&k.prev_session),
            focus_list: KeySet::parse(&k.focus_list),
            focus_pane: KeySet::parse(&k.focus_pane),
        }
    }
}

#[derive(PartialEq)]
enum Drag {
    None,
    Separator,
    Select,
    Forward,
}

pub struct App {
    pub cfg: Config,
    pub state: State,
    pub theme: &'static Theme,
    pub keys: GlobalKeys,
    pub sessions: Vec<Session>,
    pub selected: Option<RowKey>,
    pub current: Option<String>,
    pub focus: Focus,
    pub modal: Option<Modal>,
    pub tx: Sender<AppEvent>,
    pub hook_settings: PathBuf,
    pub sock: PathBuf,
    pub toast: Option<(String, Instant)>,
    pub quit: bool,
    pub app_focused: bool,
    pub tick: u64,
    pub folders: Vec<PathBuf>,
    pub list_rect: Rect,
    pub pane_rect: Rect,
    pub pane_inner: Rect,
    pub list_rows: Vec<(u16, RowKey)>,
    pub list_offset: usize,
    pub selection: Option<((u16, u16), (u16, u16))>,
    drag: Drag,
    last_badge: String,
    last_second: Instant,
    last_click: Option<(Instant, u16, u16)>,
}

fn rank(s: &Session) -> u8 {
    match s.status {
        Status::NeedsInput => 0,
        Status::Working => 1,
        Status::Done if !s.seen => 2,
        Status::Done | Status::Idle => 3,
        Status::Stopped => 4,
    }
}

impl App {
    pub fn new(tx: Sender<AppEvent>, hook_settings: PathBuf, sock: PathBuf) -> Self {
        let cfg = Config::load();
        let state = State::load();
        let sessions = state.sessions.iter().map(Session::from_record).collect();
        let mut app = Self {
            theme: theme::by_id(&cfg.theme),
            keys: GlobalKeys::from(&cfg),
            cfg,
            state,
            sessions,
            selected: None,
            current: None,
            focus: Focus::List,
            modal: None,
            tx,
            hook_settings,
            sock,
            toast: None,
            quit: false,
            app_focused: true,
            tick: 0,
            folders: vec![],
            list_rect: Rect::default(),
            pane_rect: Rect::default(),
            pane_inner: Rect::default(),
            list_rows: vec![],
            list_offset: 0,
            selection: None,
            drag: Drag::None,
            last_badge: String::new(),
            last_second: Instant::now(),
            last_click: None,
        };
        app.selected = app.rows().into_iter().next();
        app.scan_folders();
        app
    }

    // ---------- queries ----------

    pub fn session(&self, id: &str) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == id)
    }

    pub fn session_idx(&self, id: &str) -> Option<usize> {
        self.sessions.iter().position(|s| s.id == id)
    }

    pub fn project_name(&self, path: &Path) -> String {
        self.state
            .projects
            .iter()
            .find(|p| p.path == path)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| {
                path.file_name()
                    .map(|n| n.to_string_lossy().into())
                    .unwrap_or_else(|| tilde(path))
            })
    }

    pub fn project_sessions(&self, path: &Path) -> Vec<&Session> {
        let mut v: Vec<&Session> = self
            .sessions
            .iter()
            .filter(|s| s.project == path && !s.archived)
            .collect();
        v.sort_by(|a, b| rank(a).cmp(&rank(b)).then(b.created.cmp(&a.created)));
        v
    }

    /// Rows of the agent list, in display order.
    pub fn rows(&self) -> Vec<RowKey> {
        let mut projects: Vec<&ProjectEntry> = self.state.projects.iter().collect();
        projects.sort_by_key(|p| {
            if self
                .sessions
                .iter()
                .any(|s| s.project == p.path && !s.archived && s.status == Status::NeedsInput)
            {
                0
            } else {
                1
            }
        });
        let mut rows = Vec::new();
        for p in projects {
            rows.push(RowKey::Project(p.path.clone()));
            for s in self.project_sessions(&p.path) {
                rows.push(RowKey::Session(s.id.clone()));
            }
        }
        rows
    }

    /// All visible sessions in attention order.
    fn attention_order(&self) -> Vec<String> {
        let mut v: Vec<&Session> = self.sessions.iter().filter(|s| !s.archived).collect();
        v.sort_by(|a, b| rank(a).cmp(&rank(b)).then(b.created.cmp(&a.created)));
        v.into_iter().map(|s| s.id.clone()).collect()
    }

    pub fn selected_project(&self) -> Option<PathBuf> {
        match &self.selected {
            Some(RowKey::Project(p)) => Some(p.clone()),
            Some(RowKey::Session(id)) => self.session(id).map(|s| s.project.clone()),
            None => self.state.projects.first().map(|p| p.path.clone()),
        }
    }

    fn live_count(&self) -> usize {
        self.sessions.iter().filter(|s| s.pty.is_some()).count()
    }

    pub fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
    }

    // ---------- selection ----------

    fn select(&mut self, key: RowKey) {
        match &key {
            RowKey::Session(id) => {
                if self.current.as_ref() != Some(id) {
                    self.selection = None;
                }
                self.current = Some(id.clone());
            }
            RowKey::Project(p) => {
                // Show the project's top session, if any.
                let first = self.project_sessions(p).first().map(|s| s.id.clone());
                if first != self.current {
                    self.selection = None;
                }
                self.current = first;
            }
        }
        self.selected = Some(key);
        self.mark_seen();
    }

    fn focus_session(&mut self, id: &str) {
        if let Some(i) = self.session_idx(id) {
            self.sessions[i].archived = false;
        }
        self.select(RowKey::Session(id.to_string()));
        self.focus = Focus::Pane;
    }

    fn move_selection(&mut self, d: i32) {
        let rows = self.rows();
        if rows.is_empty() {
            return;
        }
        let i = self
            .selected
            .as_ref()
            .and_then(|s| rows.iter().position(|r| r == s))
            .unwrap_or(0) as i32;
        let n = (i + d).clamp(0, rows.len() as i32 - 1) as usize;
        self.select(rows[n].clone());
    }

    fn cycle_attention(&mut self, d: i32) {
        let order = self.attention_order();
        if order.is_empty() {
            return;
        }
        let i = self
            .current
            .as_ref()
            .and_then(|c| order.iter().position(|o| o == c));
        let n = match i {
            Some(i) => (i as i32 + d).rem_euclid(order.len() as i32) as usize,
            None => 0,
        };
        let id = order[n].clone();
        self.select(RowKey::Session(id));
    }

    fn mark_seen(&mut self) {
        if !self.app_focused {
            return;
        }
        if let Some(id) = &self.current
            && let Some(i) = self.session_idx(id)
        {
            self.sessions[i].seen = true;
        }
    }

    // ---------- projects ----------

    pub fn add_project(&mut self, path: PathBuf) {
        let path = path.canonicalize().unwrap_or(path);
        if self.state.projects.iter().any(|p| p.path == path) {
            self.select(RowKey::Project(path));
            return;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into())
            .unwrap_or_else(|| tilde(&path));
        self.toast(format!("Added {name}"));
        self.state.projects.push(ProjectEntry {
            path: path.clone(),
            name,
        });
        let _ = self.state.save();
        self.select(RowKey::Project(path));
        self.focus = Focus::List;
    }

    fn remove_project(&mut self, path: &Path) {
        for s in self.sessions.iter_mut().filter(|s| s.project == path) {
            s.kill();
        }
        self.sessions.retain(|s| s.project != path);
        self.state.projects.retain(|p| p.path != path);
        self.toast(format!("Removed {}", tilde(path)));
        self.save();
        self.selected = self.rows().into_iter().next();
        if let Some(k) = self.selected.clone() {
            self.select(k);
        } else {
            self.current = None;
        }
        self.focus = Focus::List;
    }

    fn request_remove_project(&mut self, path: PathBuf) {
        let live = self
            .sessions
            .iter()
            .filter(|s| s.project == path && s.pty.is_some())
            .count();
        if live > 0 {
            self.modal = Some(Modal::Confirm(Confirm {
                message: format!(
                    "{} has {live} running session{}. Stop and remove?",
                    self.project_name(&path),
                    if live == 1 { "" } else { "s" }
                ),
                action: ConfirmAction::RemoveProject(path),
            }));
        } else {
            self.remove_project(&path);
        }
    }

    fn scan_folders(&self) {
        let roots: Vec<PathBuf> = self.cfg.project_roots.iter().map(|r| expand(r)).collect();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let mut out = Vec::new();
            for root in roots {
                let Ok(rd) = std::fs::read_dir(&root) else {
                    continue;
                };
                for e in rd.flatten() {
                    let p = e.path();
                    let name = e.file_name().to_string_lossy().to_string();
                    if !p.is_dir() || name.starts_with('.') || name == "node_modules" {
                        continue;
                    }
                    out.push(p.clone());
                    // One level deeper for org/repo layouts (only git repos).
                    if !p.join(".git").exists()
                        && let Ok(rd2) = std::fs::read_dir(&p)
                    {
                        for e2 in rd2.flatten() {
                            let p2 = e2.path();
                            if p2.join(".git").exists() {
                                out.push(p2);
                            }
                        }
                    }
                }
            }
            out.sort();
            let _ = tx.send(AppEvent::Folders(out));
        });
    }

    // ---------- sessions ----------

    fn pane_size(&self) -> (u16, u16) {
        if self.pane_inner.width > 0 {
            (self.pane_inner.height, self.pane_inner.width)
        } else {
            let (w, h) = crossterm::terminal::size().unwrap_or((120, 40));
            (
                h.saturating_sub(3),
                w.saturating_sub(self.state.list_width + 2),
            )
        }
    }

    fn spawn(&mut self, idx: usize, prompt: Option<&str>, resume: bool) {
        let size = self.pane_size();
        let launch = session::Launch {
            claude: &self.cfg.claude_command,
            settings: &self.hook_settings,
            sock: &self.sock,
            prompt,
            resume,
            size,
            term_bg: Theme::rgb_of(self.theme.bg),
            term_fg: Theme::rgb_of(self.theme.fg),
        };
        if let Err(e) = self.sessions[idx].spawn(launch, self.tx.clone()) {
            self.toast(format!("Failed to start claude: {e:#}"));
        }
    }

    fn launch(&mut self, l: Launch) {
        let (cwd, branch) = match &l.tree {
            Tree::Main => (l.project.clone(), git::current_branch(&l.project)),
            Tree::Existing { path, branch } => (path.clone(), Some(branch.clone())),
            Tree::New => {
                let pname = self.project_name(&l.project);
                let dir = expand(&self.cfg.worktree_dir)
                    .join(&pname)
                    .join(l.branch.replace('/', "-"));
                if let Err(e) = git::add_worktree(&l.project, &l.branch, &dir) {
                    self.toast(format!("git worktree failed: {e}"));
                    return;
                }
                (dir, Some(l.branch.clone()))
            }
        };
        let name: String = l
            .prompt
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(60)
            .collect();
        let s = Session::new(l.project.clone(), cwd, branch, name, l.model, l.effort);
        let id = s.id.clone();
        self.sessions.push(s);
        let idx = self.sessions.len() - 1;
        let prompt = (!l.prompt.is_empty()).then_some(l.prompt.as_str());
        self.spawn(idx, prompt, false);
        self.save();
        self.focus_session(&id);
    }

    fn resume(&mut self, id: &str) {
        if let Some(i) = self.session_idx(id)
            && self.sessions[i].pty.is_none()
        {
            self.spawn(i, None, true);
            self.sessions[i].archived = false;
        }
    }

    fn request_close_session(&mut self, id: String) {
        let Some(s) = self.session(&id) else { return };
        if s.pty.is_some() && matches!(s.status, Status::Working | Status::NeedsInput) {
            self.modal = Some(Modal::Confirm(Confirm {
                message: format!(
                    "Stop \"{}\"? (resumable later)",
                    if s.name.is_empty() {
                        "new session"
                    } else {
                        &s.name
                    }
                ),
                action: ConfirmAction::CloseSession(id),
            }));
        } else {
            self.close_session(&id);
        }
    }

    fn close_session(&mut self, id: &str) {
        let rows = self.rows();
        let pos = rows
            .iter()
            .position(|r| *r == RowKey::Session(id.to_string()));
        if let Some(i) = self.session_idx(id) {
            self.sessions[i].kill();
            self.sessions[i].archived = true;
        }
        if self.current.as_deref() == Some(id) {
            self.current = None;
        }
        // Move selection to the neighbouring row.
        let rows = self.rows();
        if let Some(p) = pos
            && let Some(k) = rows.get(p.min(rows.len().saturating_sub(1))).cloned()
        {
            self.select(k);
        }
        self.focus = Focus::List;
    }

    pub fn save(&mut self) {
        let mut recs: Vec<_> = self.sessions.iter().map(|s| s.record()).collect();
        recs.sort_by_key(|r| std::cmp::Reverse(r.created));
        recs.truncate(200);
        self.state.sessions = recs;
        let _ = self.state.save();
    }

    pub fn shutdown(&mut self) {
        self.save();
        for s in &mut self.sessions {
            s.kill();
        }
        if self.cfg.notifications.badge {
            notify::badge("");
        }
    }

    // ---------- events ----------

    pub fn handle(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::Input(e) => self.handle_input(e),
            AppEvent::Output => {}
            AppEvent::Exited(id, generation) => {
                if let Some(i) = self.session_idx(&id) {
                    let s = &mut self.sessions[i];
                    if s.generation == generation {
                        let quick = s.last_change.elapsed() < Duration::from_secs(2)
                            && s.status != Status::Stopped;
                        s.pty = None;
                        s.set_status(Status::Stopped);
                        if quick {
                            self.toast("claude exited immediately — check the pane for errors");
                        }
                    }
                }
            }
            AppEvent::Hook(m) => self.handle_hook(m),
            AppEvent::Folders(f) => self.folders = f,
        }
    }

    fn handle_hook(&mut self, m: HookMsg) {
        let Some(i) = self.session_idx(&m.sid) else {
            return;
        };
        let prev = self.sessions[i].status;
        let next = match m.event.as_str() {
            "UserPromptSubmit" => {
                if self.sessions[i].name.is_empty() && !m.prompt.trim().is_empty() {
                    self.sessions[i].name = m
                        .prompt
                        .lines()
                        .next()
                        .unwrap_or("")
                        .chars()
                        .take(60)
                        .collect();
                }
                Some(Status::Working)
            }
            "PreToolUse" if matches!(m.tool.as_str(), "AskUserQuestion" | "ExitPlanMode") => {
                Some(Status::NeedsInput)
            }
            "PreToolUse" | "PostToolUse" => Some(Status::Working),
            "Notification" => match m.ntype.as_str() {
                "permission_prompt" | "elicitation_dialog" => Some(Status::NeedsInput),
                "idle_prompt" | "auth_success" => None,
                _ if m.message.to_lowercase().contains("permission") => Some(Status::NeedsInput),
                _ => None,
            },
            "Stop" => Some(Status::Done),
            _ => None,
        };
        let Some(next) = next else { return };
        let visible =
            self.app_focused && self.current.as_deref() == Some(&m.sid) && self.modal.is_none();
        let s = &mut self.sessions[i];
        if prev == Status::Stopped {
            return;
        }
        s.set_status(next);
        if next == Status::Done {
            s.seen = visible;
        }
        if next != prev && matches!(next, Status::NeedsInput | Status::Done) && !visible {
            self.alert(i);
        }
    }

    fn alert(&mut self, i: usize) {
        let s = &mut self.sessions[i];
        s.last_notified = Some(Instant::now());
        let project = self
            .state
            .projects
            .iter()
            .find(|p| p.path == s.project)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        let what = if s.status == Status::NeedsInput {
            "Needs your input"
        } else {
            "Finished"
        };
        let name = if s.name.is_empty() {
            "session".to_string()
        } else {
            s.name.clone()
        };
        if self.cfg.notifications.macos {
            notify::macos(&format!("maximus · {project}"), &format!("{what}: {name}"));
        }
        if self.cfg.notifications.bell {
            notify::bell();
        }
    }

    /// Called ~10x/second. Returns true if a redraw is needed.
    pub fn on_tick(&mut self) -> bool {
        self.tick = self.tick.wrapping_add(1);
        let mut redraw = self.sessions.iter().any(|s| s.status == Status::Working);
        if let Some((_, at)) = &self.toast
            && at.elapsed() > Duration::from_secs(4)
        {
            self.toast = None;
            redraw = true;
        }
        if self.last_second.elapsed() >= Duration::from_secs(1) {
            self.last_second = Instant::now();
            redraw |= self.timeouts();
            self.update_badge();
        }
        redraw
    }

    fn timeouts(&mut self) -> bool {
        let t = self.cfg.timeouts.clone();
        let mins = |m: u32| Duration::from_secs(m as u64 * 60);
        let mut changed = false;
        let mut nag = Vec::new();
        for (i, s) in self.sessions.iter_mut().enumerate() {
            let idle = s.last_change.elapsed();
            let is_current = self.current.as_deref() == Some(&s.id);
            if t.idle_kill > 0
                && s.pty.is_some()
                && matches!(s.status, Status::Done | Status::Idle)
                && idle > mins(t.idle_kill)
            {
                s.kill();
                changed = true;
            }
            if t.archive_finished > 0
                && !s.archived
                && !is_current
                && (matches!(s.status, Status::Stopped) || (s.status == Status::Done && s.seen))
                && idle > mins(t.archive_finished)
            {
                s.archived = true;
                changed = true;
            }
            if t.needs_input_nag > 0
                && s.status == Status::NeedsInput
                && s.last_notified.map(|n| n.elapsed()).unwrap_or(idle) > mins(t.needs_input_nag)
                && idle > mins(t.needs_input_nag)
            {
                nag.push(i);
            }
        }
        for i in nag {
            self.alert(i);
        }
        if changed
            && let Some(sel) = self.selected.clone()
            && !self.rows().contains(&sel)
        {
            self.selected = self.rows().into_iter().next();
        }
        changed
    }

    pub fn update_badge(&mut self) {
        if !self.cfg.notifications.badge {
            return;
        }
        let waiting = self
            .sessions
            .iter()
            .filter(|s| s.status == Status::NeedsInput)
            .count();
        let text = if waiting > 0 {
            format!("{waiting} waiting")
        } else {
            String::new()
        };
        if text != self.last_badge {
            notify::badge(&text);
            self.last_badge = text;
        }
    }

    fn handle_input(&mut self, e: Event) {
        match e {
            Event::Key(k) if k.kind != KeyEventKind::Release => self.handle_key(k),
            Event::Mouse(m) => self.handle_mouse(m),
            Event::Paste(s) => self.handle_paste(&s),
            Event::FocusGained => {
                self.app_focused = true;
                self.mark_seen();
            }
            Event::FocusLost => self.app_focused = false,
            _ => {}
        }
    }

    fn handle_paste(&mut self, s: &str) {
        match &mut self.modal {
            Some(Modal::Switcher(sw)) => sw.paste(s),
            Some(Modal::Prompt(p)) => p.paste(s),
            Some(Modal::Settings(st)) => st.paste(s),
            Some(_) => {}
            None => {
                if self.focus == Focus::Pane
                    && let Some(i) = self.current.clone().and_then(|c| self.session_idx(&c))
                {
                    let bracketed = self.sessions[i]
                        .parser
                        .lock()
                        .unwrap()
                        .screen()
                        .bracketed_paste();
                    let mut bytes = Vec::new();
                    if bracketed {
                        bytes.extend_from_slice(b"\x1b[200~");
                    }
                    bytes.extend_from_slice(s.replace("\r\n", "\r").replace('\n', "\r").as_bytes());
                    if bracketed {
                        bytes.extend_from_slice(b"\x1b[201~");
                    }
                    self.sessions[i].scroll = 0;
                    self.sessions[i].write(&bytes);
                }
            }
        }
    }

    // ---------- modals ----------

    fn open_switcher(&mut self) {
        let t = self.theme;
        let mut items = Vec::new();
        let mut sessions: Vec<&Session> = self.sessions.iter().collect();
        sessions.sort_by(|a, b| {
            a.archived
                .cmp(&b.archived)
                .then(rank(a).cmp(&rank(b)))
                .then(b.created.cmp(&a.created))
        });
        for s in sessions {
            let (dot, color) = ui::status_dot(s, self.tick, t);
            let mut detail = self.project_name(&s.project);
            if let Some(b) = &s.branch {
                detail += &format!(" · {b}");
            }
            detail += &format!(" · {}", ui::status_word(s));
            items.push(Item {
                label: if s.name.is_empty() {
                    "new session".into()
                } else {
                    s.name.clone()
                },
                detail,
                tag: if s.archived { "archived" } else { "session" },
                dot: Some((dot, color)),
                action: Action::Session(s.id.clone()),
            });
        }
        for p in &self.state.projects {
            items.push(Item {
                label: p.name.clone(),
                detail: tilde(&p.path),
                tag: "project",
                dot: Some(("▣".into(), t.accent)),
                action: Action::Project(p.path.clone()),
            });
        }
        let cmd = |label: String, c: Cmd| Item {
            label,
            detail: String::new(),
            tag: "command",
            dot: Some(("›".into(), t.muted)),
            action: Action::Cmd(c),
        };
        let k = &self.keys;
        items.push(Item {
            detail: k.new_prompt.short(),
            ..cmd("New prompt".into(), Cmd::NewPrompt)
        });
        items.push(cmd("New prompt in a new worktree".into(), Cmd::NewWorktree));
        items.push(Item {
            detail: k.diff.short(),
            ..cmd("View changes (diff)".into(), Cmd::Diff)
        });
        items.push(Item {
            detail: k.settings.short(),
            ..cmd("Settings".into(), Cmd::Settings)
        });
        if let Some(id) = &self.current
            && let Some(s) = self.session(id)
        {
            if s.pty.is_some() {
                items.push(cmd(
                    format!("Stop session: {}", s.name),
                    Cmd::CloseSession(id.clone()),
                ));
            } else {
                items.push(cmd(
                    format!("Resume session: {}", s.name),
                    Cmd::ResumeSession(id.clone()),
                ));
            }
        }
        for p in &self.state.projects {
            items.push(cmd(
                format!("Remove project: {}", p.name),
                Cmd::RemoveProject(p.path.clone()),
            ));
        }
        for m in &self.cfg.models {
            items.push(cmd(
                format!("Default model: {m}"),
                Cmd::DefaultModel(m.clone()),
            ));
        }
        for th in THEMES {
            items.push(cmd(format!("Theme: {}", th.label), Cmd::Theme(th.id)));
        }
        items.push(cmd("Quit".into(), Cmd::Quit));

        let folders = self
            .folders
            .iter()
            .filter(|f| !self.state.projects.iter().any(|p| &p.path == *f))
            .map(|f| Item {
                label: f
                    .file_name()
                    .map(|n| n.to_string_lossy().into())
                    .unwrap_or_default(),
                detail: tilde(f),
                tag: "add folder",
                dot: Some(("+".into(), t.green)),
                action: Action::AddFolder(f.clone()),
            })
            .collect();
        self.scan_folders();
        self.modal = Some(Modal::Switcher(Box::new(Switcher::new(items, folders))));
    }

    fn open_prompt(&mut self, new_worktree: bool) {
        let Some(project) = self.selected_project() else {
            self.toast("Add a project first");
            self.open_switcher();
            return;
        };
        let p = Prompt::new(
            project,
            self.state.projects.clone(),
            self.cfg.models.clone(),
            self.cfg.default_model.clone(),
            self.cfg.default_effort.clone(),
        );
        self.modal = Some(Modal::Prompt(if new_worktree {
            p.with_new_worktree()
        } else {
            p
        }));
    }

    fn open_diff(&mut self) {
        let dir = self
            .current
            .as_ref()
            .and_then(|c| self.session(c))
            .map(|s| s.cwd.clone())
            .or_else(|| self.selected_project());
        let Some(dir) = dir else {
            self.toast("Nothing to diff");
            return;
        };
        if !git::is_repo(&dir) {
            self.toast(format!("{} is not a git repository", tilde(&dir)));
            return;
        }
        self.modal = Some(Modal::Diff(Box::new(DiffView::open(
            dir,
            self.theme,
            self.state.diff_tree_width,
            self.state.diff_split,
        ))));
    }

    fn close_modal(&mut self) {
        if let Some(Modal::Diff(d)) = &self.modal {
            self.state.diff_tree_width = d.tree_width;
            self.state.diff_split = d.split;
            let _ = self.state.save();
        }
        self.modal = None;
        self.mark_seen();
    }

    fn run_action(&mut self, a: Action) {
        self.modal = None;
        match a {
            Action::Session(id) => {
                self.focus_session(&id);
                if self.session(&id).is_some_and(|s| s.pty.is_none()) {
                    self.focus = Focus::List;
                }
            }
            Action::Project(p) => {
                self.select(RowKey::Project(p));
                self.focus = Focus::List;
            }
            Action::AddFolder(p) => self.add_project(p),
            Action::Cmd(c) => match c {
                Cmd::NewPrompt => self.open_prompt(false),
                Cmd::NewWorktree => self.open_prompt(true),
                Cmd::Diff => self.open_diff(),
                Cmd::Settings => self.modal = Some(Modal::Settings(SettingsModal::new())),
                Cmd::RemoveProject(p) => self.request_remove_project(p),
                Cmd::CloseSession(id) => self.request_close_session(id),
                Cmd::ResumeSession(id) => {
                    self.resume(&id);
                    self.focus_session(&id);
                }
                Cmd::DefaultModel(m) => {
                    self.cfg.default_model = m.clone();
                    let _ = self.cfg.save();
                    self.toast(format!("Default model: {m}"));
                }
                Cmd::Theme(id) => {
                    self.cfg.theme = id.into();
                    self.theme = theme::by_id(id);
                    let _ = self.cfg.save();
                }
                Cmd::Quit => self.request_quit(),
            },
        }
    }

    fn request_quit(&mut self) {
        let live = self.live_count();
        if live > 0 {
            self.modal = Some(Modal::Confirm(Confirm {
                message: format!(
                    "{live} session{} running. Stop and quit?",
                    if live == 1 { "" } else { "s" }
                ),
                action: ConfirmAction::Quit,
            }));
        } else {
            self.quit = true;
        }
    }

    fn settings_changed(&mut self) {
        let _ = self.cfg.save();
        self.theme = theme::by_id(&self.cfg.theme);
        self.keys = GlobalKeys::from(&self.cfg);
    }

    // ---------- keys ----------

    fn handle_key(&mut self, k: KeyEvent) {
        if let Some(modal) = &mut self.modal {
            match modal {
                Modal::Switcher(s) => match s.handle_key(k) {
                    Err(()) => self.close_modal(),
                    Ok(Some(a)) => self.run_action(a),
                    Ok(None) => {}
                },
                Modal::Prompt(p) => match p.handle_key(k) {
                    PromptAction::Close => self.close_modal(),
                    PromptAction::Launch(l) => {
                        self.modal = None;
                        self.launch(l);
                    }
                    PromptAction::None => {}
                },
                Modal::Diff(d) => {
                    if self.keys.diff.matches(&k) {
                        self.close_modal();
                    } else if let DiffAction::Close = d.handle_key(k) {
                        self.close_modal();
                    }
                }
                Modal::Settings(s) => match s.handle_key(k, &mut self.cfg) {
                    SettingsAction::Close => self.close_modal(),
                    SettingsAction::Changed => self.settings_changed(),
                    SettingsAction::None => {}
                },
                Modal::Confirm(c) => match k.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                        let Some(Modal::Confirm(c)) = self.modal.take() else {
                            return;
                        };
                        match c.action {
                            ConfirmAction::Quit => self.quit = true,
                            ConfirmAction::RemoveProject(p) => self.remove_project(&p),
                            ConfirmAction::CloseSession(id) => self.close_session(&id),
                        }
                    }
                    KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc | KeyCode::Char('q') => {
                        let _ = c;
                        self.modal = None;
                    }
                    _ => {}
                },
            }
            return;
        }

        // Global keys win, even inside a pane.
        let g = &self.keys;
        if g.switcher.matches(&k) {
            return self.open_switcher();
        }
        if g.new_prompt.matches(&k) {
            return self.open_prompt(false);
        }
        if g.diff.matches(&k) {
            return self.open_diff();
        }
        if g.settings.matches(&k) {
            self.modal = Some(Modal::Settings(SettingsModal::new()));
            return;
        }
        if g.next_session.matches(&k) {
            return self.cycle_attention(1);
        }
        if g.prev_session.matches(&k) {
            return self.cycle_attention(-1);
        }
        if g.focus_list.matches(&k) {
            self.focus = Focus::List;
            return;
        }
        if g.focus_pane.matches(&k) {
            if self.current.is_some() {
                self.focus = Focus::Pane;
            }
            return;
        }

        match self.focus {
            Focus::Pane => self.pane_key(k),
            Focus::List => self.list_key(k),
        }
    }

    fn pane_key(&mut self, k: KeyEvent) {
        let Some(i) = self.current.clone().and_then(|c| self.session_idx(&c)) else {
            self.focus = Focus::List;
            return;
        };
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let rows = self.pane_inner.height.max(1) as usize;
        if self.sessions[i].pty.is_none() {
            match k.code {
                KeyCode::Enter | KeyCode::Char('r') => {
                    let id = self.sessions[i].id.clone();
                    self.resume(&id);
                }
                KeyCode::Esc => self.focus = Focus::List,
                _ => {}
            }
            return;
        }
        match k.code {
            KeyCode::PageUp if shift => {
                self.sessions[i].scroll += rows / 2;
                return;
            }
            KeyCode::PageDown if shift => {
                self.sessions[i].scroll = self.sessions[i].scroll.saturating_sub(rows / 2);
                return;
            }
            _ => {}
        }
        self.selection = None;
        let s = &mut self.sessions[i];
        s.scroll = 0;
        let app_cursor = s.parser.lock().unwrap().screen().application_cursor();
        let bytes = keys::encode(&k, app_cursor);
        if !bytes.is_empty() {
            s.write(&bytes);
        }
    }

    fn list_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Char('g') | KeyCode::Home => self.move_selection(-10_000),
            KeyCode::Char('G') | KeyCode::End => self.move_selection(10_000),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => match self.selected.clone() {
                Some(RowKey::Session(id)) => {
                    if self.session(&id).is_some_and(|s| s.pty.is_none()) {
                        self.resume(&id);
                    }
                    self.focus_session(&id);
                }
                Some(RowKey::Project(_)) => self.open_prompt(false),
                None => self.open_switcher(),
            },
            KeyCode::Char('n') => self.open_prompt(false),
            KeyCode::Char('w') => self.open_prompt(true),
            KeyCode::Char('r') => {
                if let Some(RowKey::Session(id)) = self.selected.clone() {
                    self.resume(&id);
                    self.focus_session(&id);
                }
            }
            KeyCode::Char('x') | KeyCode::Delete | KeyCode::Backspace => {
                match self.selected.clone() {
                    Some(RowKey::Session(id)) => self.request_close_session(id),
                    Some(RowKey::Project(p))
                        if k.code == KeyCode::Delete || k.code == KeyCode::Char('x') =>
                    {
                        self.modal = Some(Modal::Confirm(Confirm {
                            message: format!(
                                "Remove {} from maximus? (no files are deleted)",
                                self.project_name(&p)
                            ),
                            action: ConfirmAction::RemoveProject(p),
                        }));
                    }
                    _ => {}
                }
            }
            KeyCode::Char('d') => self.open_diff(),
            KeyCode::Char('/') => self.open_switcher(),
            KeyCode::Char(',') => self.modal = Some(Modal::Settings(SettingsModal::new())),
            KeyCode::Char('q') => self.request_quit(),
            KeyCode::Char('c') if ctrl => self.request_quit(),
            KeyCode::Tab if self.current.is_some() => {
                self.focus = Focus::Pane;
            }
            _ => {}
        }
    }

    // ---------- mouse ----------

    fn handle_mouse(&mut self, m: MouseEvent) {
        if let Some(modal) = &mut self.modal {
            match modal {
                Modal::Diff(d) => d.handle_mouse(m),
                Modal::Switcher(s) => {
                    if let Some(a) = s.handle_mouse(m) {
                        self.run_action(a);
                    }
                }
                Modal::Settings(s) => {
                    if let SettingsAction::Changed = s.handle_mouse(m, &mut self.cfg) {
                        self.settings_changed();
                    }
                }
                _ => {}
            }
            return;
        }
        let (x, y) = (m.column, m.row);
        let inside = |r: Rect| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height;
        let sep = self.list_rect.x + self.list_rect.width;
        let in_pane_inner = inside(self.pane_inner);
        let cur = self.current.clone().and_then(|c| self.session_idx(&c));
        let mouse_mode = cur
            .filter(|&i| self.sessions[i].pty.is_some())
            .map(|i| {
                self.sessions[i]
                    .parser
                    .lock()
                    .unwrap()
                    .screen()
                    .mouse_protocol_mode()
            })
            .filter(|m| *m != vt100::MouseProtocolMode::None);

        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if (x + 1 == sep || x == sep)
                    && y >= self.list_rect.y
                    && y < self.list_rect.y + self.list_rect.height
                {
                    self.drag = Drag::Separator;
                } else if inside(self.list_rect) {
                    self.focus = Focus::List;
                    if let Some((_, key)) = self.list_rows.iter().find(|(ry, _)| *ry == y).cloned()
                    {
                        let double = self.last_click.is_some_and(|(t, cx, cy)| {
                            t.elapsed() < Duration::from_millis(400) && cx == x && cy == y
                        });
                        let again = self.selected.as_ref() == Some(&key);
                        self.select(key.clone());
                        if (double || again)
                            && let RowKey::Session(id) = key
                        {
                            if self.session(&id).is_some_and(|s| s.pty.is_none()) {
                                self.resume(&id);
                            }
                            self.focus = Focus::Pane;
                        }
                    }
                    self.last_click = Some((Instant::now(), x, y));
                } else if in_pane_inner && cur.is_some() {
                    self.focus = Focus::Pane;
                    if mouse_mode.is_some() {
                        self.forward_mouse(m);
                        self.drag = Drag::Forward;
                    } else {
                        let p = (x - self.pane_inner.x, y - self.pane_inner.y);
                        self.selection = Some((p, p));
                        self.drag = Drag::Select;
                    }
                } else if inside(self.pane_rect) && cur.is_some() {
                    self.focus = Focus::Pane;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => match self.drag {
                Drag::Separator => {
                    let w = x.saturating_sub(self.list_rect.x) + 1;
                    self.state.list_width =
                        w.clamp(18, self.list_rect.width + self.pane_rect.width - 30);
                }
                Drag::Select => {
                    let px = x.clamp(
                        self.pane_inner.x,
                        self.pane_inner.x + self.pane_inner.width.saturating_sub(1),
                    );
                    let py = y.clamp(
                        self.pane_inner.y,
                        self.pane_inner.y + self.pane_inner.height.saturating_sub(1),
                    );
                    if let Some((a, _)) = self.selection {
                        self.selection =
                            Some((a, (px - self.pane_inner.x, py - self.pane_inner.y)));
                    }
                }
                Drag::Forward => self.forward_mouse(m),
                Drag::None => {}
            },
            MouseEventKind::Up(_) => {
                match self.drag {
                    Drag::Separator => {
                        let _ = self.state.save();
                    }
                    Drag::Select => self.copy_selection(),
                    Drag::Forward => self.forward_mouse(m),
                    Drag::None => {}
                }
                self.drag = Drag::None;
            }
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let up = m.kind == MouseEventKind::ScrollUp;
                if inside(self.list_rect) {
                    self.move_selection(if up { -1 } else { 1 });
                } else if inside(self.pane_rect) {
                    let Some(i) = cur else { return };
                    if mouse_mode.is_some() && in_pane_inner {
                        self.forward_mouse(m);
                    } else if self.sessions[i]
                        .parser
                        .lock()
                        .unwrap()
                        .screen()
                        .alternate_screen()
                    {
                        let seq: &[u8] = if up { b"\x1b[A" } else { b"\x1b[B" };
                        self.sessions[i].write(&seq.repeat(3));
                    } else if up {
                        self.sessions[i].scroll += 3;
                    } else {
                        self.sessions[i].scroll = self.sessions[i].scroll.saturating_sub(3);
                    }
                }
            }
            _ => {}
        }
    }

    fn forward_mouse(&mut self, m: MouseEvent) {
        let Some(i) = self.current.clone().and_then(|c| self.session_idx(&c)) else {
            return;
        };
        let (x, y) = (
            m.column.saturating_sub(self.pane_inner.x) + 1,
            m.row.saturating_sub(self.pane_inner.y) + 1,
        );
        let button = |b: MouseButton| match b {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
        };
        let (code, release) = match m.kind {
            MouseEventKind::Down(b) => (button(b), false),
            MouseEventKind::Up(b) => (button(b), true),
            MouseEventKind::Drag(b) => (button(b) + 32, false),
            MouseEventKind::ScrollUp => (64, false),
            MouseEventKind::ScrollDown => (65, false),
            _ => return,
        };
        let mut code = code;
        if m.modifiers.contains(KeyModifiers::SHIFT) {
            code += 4;
        }
        if m.modifiers.contains(KeyModifiers::ALT) {
            code += 8;
        }
        if m.modifiers.contains(KeyModifiers::CONTROL) {
            code += 16;
        }
        let seq = format!("\x1b[<{code};{x};{y}{}", if release { 'm' } else { 'M' });
        self.sessions[i].write(seq.as_bytes());
    }

    fn copy_selection(&mut self) {
        let Some((a, b)) = self.selection else { return };
        if a == b {
            self.selection = None;
            return;
        }
        let ((sc, sr), (ec, er)) = if (a.1, a.0) <= (b.1, b.0) {
            (a, b)
        } else {
            (b, a)
        };
        let Some(i) = self.current.clone().and_then(|c| self.session_idx(&c)) else {
            return;
        };
        let text = {
            let p = self.sessions[i].parser.lock().unwrap();
            p.screen().contents_between(sr, sc, er, ec + 1)
        };
        let text: String = text
            .lines()
            .map(|l| l.trim_end())
            .collect::<Vec<_>>()
            .join("\n");
        if !text.trim().is_empty() {
            notify::copy(&text);
            self.toast(format!("Copied {} chars", text.chars().count()));
        }
    }
}
