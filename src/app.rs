use std::{
    cmp::Ordering,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use crossbeam_channel::Sender;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;

use crate::{
    config::{Config, ListSort, ProjectEntry, State, expand, tilde},
    diff::{DiffAction, DiffView, FilePreview},
    git::{self, GraphRow},
    hooks::HookMsg,
    keys::{self, KeySet},
    memory::{self, Usage},
    newproject::{NewProject, NewProjectAction},
    notify,
    prompt::{Launch, Prompt, PromptAction, Tree},
    session::{self, Session, Status},
    settings::{SettingsAction, SettingsModal},
    switcher::{Action, Cmd, Item, Switcher},
    terminal::Terminal,
    textinput::TextInput,
    theme::{self, THEMES, Theme},
    ui,
};

pub enum AppEvent {
    Input(Event),
    Output,
    Exited(String, u64),
    Hook(HookMsg),
    Folders(Vec<PathBuf>),
    Graph(PathBuf, Vec<GraphRow>),
    /// A background commit in this checkout finished: the short hash, or git's error.
    Committed(PathBuf, Result<String, String>),
    /// The shell of the terminal with this id exited.
    ShellExited(u64),
    /// A memory measurement for the memory modal.
    Memory(Usage),
}

#[derive(Debug, Clone, PartialEq)]
pub enum RowKey {
    Project(PathBuf),
    Session(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Focus {
    List,
    Graph,
    Pane,
}

pub enum ConfirmAction {
    Quit,
    RemoveProject(PathBuf),
    CloseSession(String),
    FinishWorktree {
        project: PathBuf,
        wt: git::Worktree,
        merge: bool,
    },
}

pub struct Confirm {
    pub message: String,
    pub action: ConfirmAction,
}

pub struct Rename {
    pub id: String,
    pub input: TextInput,
}

// Only one modal exists at a time, so variant size differences don't matter.
#[allow(clippy::large_enum_variant)]
pub enum Modal {
    Switcher(Box<Switcher>),
    Prompt(Prompt),
    Diff(Box<DiffView>),
    Settings(SettingsModal),
    Confirm(Confirm),
    Rename(Rename),
    NewProject(NewProject),
    /// Shows the terminal with this id.
    Terminal(u64),
    /// Memory use of sessions, shells and maximus; None until the first measurement.
    Memory(Option<Usage>),
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
    pub graph: KeySet,
    pub commit: KeySet,
    pub terminal: KeySet,
    pub memory: KeySet,
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
            graph: KeySet::parse(&k.graph),
            commit: KeySet::parse(&k.commit),
            terminal: KeySet::parse(&k.terminal),
            memory: KeySet::parse(&k.memory),
        }
    }
}

/// Commit graph for the checkout behind the current session or selected project.
#[derive(Default)]
pub struct Graph {
    pub dir: Option<PathBuf>,
    pub rows: Vec<GraphRow>,
    pub loaded: bool,
    /// Selected row (always a commit row once loaded) and first visible line.
    pub sel: usize,
    pub offset: usize,
    /// The selected commit's changed files, shown under it unless collapsed with esc.
    pub files: Vec<git::Change>,
    files_of: Option<String>,
    pub collapsed: bool,
    /// Selected file under the commit; None while the commit row itself is selected.
    pub file: Option<usize>,
    pub preview: Option<FilePreview>,
    pending: bool,
    fetched: Option<Instant>,
}

/// One line of the graph panel.
#[derive(Clone, Copy, PartialEq)]
pub enum GraphLine {
    Row(usize),
    File(usize),
}

impl Graph {
    /// Moves the selection `d` commits, skipping the graph's connector-only lines.
    fn step(&mut self, d: i32) {
        let commits: Vec<usize> = (0..self.rows.len())
            .filter(|&i| self.rows[i].commit.is_some())
            .collect();
        let Some(cur) = commits.iter().position(|&i| i >= self.sel) else {
            return;
        };
        let next = (cur as i64 + d as i64).clamp(0, commits.len() as i64 - 1) as usize;
        if commits[next] != self.sel {
            self.sel = commits[next];
            self.collapsed = false;
            self.file = None;
        } else if d == 0 {
            self.sel = commits[next];
        }
    }

    pub fn selected_hash(&self) -> Option<&str> {
        self.rows
            .get(self.sel)
            .and_then(|r| r.commit.as_ref())
            .map(|c| c.hash.as_str())
    }

    pub fn expanded(&self) -> bool {
        !self.collapsed && self.files_of.as_deref() == self.selected_hash()
    }

    /// The panel's lines, with the selected commit's files under it when expanded.
    pub fn lines(&self) -> Vec<GraphLine> {
        let mut out = Vec::with_capacity(self.rows.len() + self.files.len());
        for i in 0..self.rows.len() {
            out.push(GraphLine::Row(i));
            if i == self.sel && self.expanded() {
                out.extend((0..self.files.len()).map(GraphLine::File));
            }
        }
        out
    }

    pub fn cursor(&self) -> GraphLine {
        match self.file {
            Some(f) if self.expanded() => GraphLine::File(f),
            _ => GraphLine::Row(self.sel),
        }
    }

    /// j/k: walk down into the expanded commit's files, then on to the next commit.
    fn down(&mut self) {
        match self.file {
            Some(f) if f + 1 < self.files.len() => self.file = Some(f + 1),
            None if self.expanded() && !self.files.is_empty() => self.file = Some(0),
            _ => self.step(1),
        }
    }

    fn up(&mut self) {
        match self.file {
            Some(0) => self.file = None,
            Some(f) => self.file = Some(f - 1),
            None => self.step(-1),
        }
    }

    /// Loads the selected commit's file list and the selected file's diff when they changed.
    fn sync(&mut self, theme: &Theme, split: bool) {
        let (Some(dir), Some(hash)) = (self.dir.clone(), self.selected_hash().map(String::from))
        else {
            return;
        };
        if !self.collapsed && self.files_of.as_deref() != Some(hash.as_str()) {
            self.files = git::commit_files(&dir, &hash).unwrap_or_default();
            self.files_of = Some(hash.clone());
            self.file = None;
        }
        let want = self
            .file
            .filter(|_| self.expanded())
            .and_then(|i| self.files.get(i));
        let Some(ch) = want else {
            self.preview = None;
            return;
        };
        let title = match &ch.old_path {
            Some(old) => format!("{old} → {} @ {hash}", ch.path),
            None => format!("{} @ {hash}", ch.path),
        };
        if self.preview.as_ref().is_some_and(|p| p.title == title) {
            return;
        }
        let old = match ch.status {
            'A' => None,
            _ => git::show_file(
                &dir,
                &format!("{hash}^"),
                ch.old_path.as_ref().unwrap_or(&ch.path),
            ),
        };
        let new = match ch.status {
            'D' => None,
            _ => git::show_file(&dir, &hash, &ch.path),
        };
        self.preview = Some(FilePreview::new(title, &ch.path, old, new, theme, split));
    }
}

#[derive(PartialEq)]
enum Drag {
    None,
    Separator,
    GraphSeparator,
    Select,
    Forward,
}

pub struct App {
    pub cfg: Config,
    pub state: State,
    pub theme: &'static Theme,
    pub keys: GlobalKeys,
    pub sessions: Vec<Session>,
    /// Shells opened with the terminal key, shown or hidden; at most one per checkout.
    pub terminals: Vec<Terminal>,
    next_terminal: u64,
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
    pub graph: Graph,
    pub list_rect: Rect,
    pub graph_rect: Rect,
    pub pane_rect: Rect,
    pub pane_inner: Rect,
    pub list_rows: Vec<(u16, RowKey)>,
    pub list_offset: usize,
    pub selection: Option<((u16, u16), (u16, u16))>,
    drag: Drag,
    last_badge: String,
    last_second: Instant,
    last_click: Option<(Instant, u16, u16)>,
    /// When the memory modal last asked for a measurement.
    memory_fetched: Option<Instant>,
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

/// Case-insensitive comparison that orders runs of digits by value, so `agent 2` < `agent 10`.
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek(), b.peek()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let num = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut d = String::new();
                    while let Some(c) = it.next_if(char::is_ascii_digit) {
                        d.push(c);
                    }
                    d.trim_start_matches('0').to_string()
                };
                let (x, y) = (num(&mut a), num(&mut b));
                let o = x.len().cmp(&y.len()).then_with(|| x.cmp(&y));
                if o.is_ne() {
                    return o;
                }
            }
            (Some(_), Some(_)) => {
                let (x, y) = (a.next().unwrap(), b.next().unwrap());
                let o = x.to_lowercase().cmp(y.to_lowercase());
                if o.is_ne() {
                    return o;
                }
            }
        }
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
            terminals: vec![],
            next_terminal: 0,
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
            graph: Graph::default(),
            list_rect: Rect::default(),
            graph_rect: Rect::default(),
            pane_rect: Rect::default(),
            pane_inner: Rect::default(),
            list_rows: vec![],
            list_offset: 0,
            selection: None,
            drag: Drag::None,
            last_badge: String::new(),
            last_second: Instant::now(),
            last_click: None,
            memory_fetched: None,
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
        match self.state.list_sort {
            ListSort::Attention => {
                v.sort_by(|a, b| rank(a).cmp(&rank(b)).then(b.created.cmp(&a.created)))
            }
            ListSort::NameAsc => v.sort_by(|a, b| natural_cmp(a.label(), b.label())),
            ListSort::NameDesc => v.sort_by(|a, b| natural_cmp(b.label(), a.label())),
        }
        v
    }

    /// Rows of the agent list, in display order.
    pub fn rows(&self) -> Vec<RowKey> {
        let mut projects: Vec<&ProjectEntry> = self.state.projects.iter().collect();
        match self.state.list_sort {
            ListSort::Attention => {
                projects.sort_by_key(|p| {
                    if self.sessions.iter().any(|s| {
                        s.project == p.path && !s.archived && s.status == Status::NeedsInput
                    }) {
                        0
                    } else {
                        1
                    }
                })
            }
            ListSort::NameAsc => projects.sort_by(|a, b| {
                natural_cmp(&self.project_name(&a.path), &self.project_name(&b.path))
            }),
            ListSort::NameDesc => projects.sort_by(|a, b| {
                natural_cmp(&self.project_name(&b.path), &self.project_name(&a.path))
            }),
        }
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

    /// The checkout the graph, diff viewer and terminal follow: the current session's, else
    /// the selected project.
    fn checkout_dir(&self) -> Option<PathBuf> {
        self.current
            .as_ref()
            .and_then(|c| self.session(c))
            .map(|s| s.cwd.clone())
            .or_else(|| self.selected_project())
    }

    /// Starts a background `git log` when the followed checkout changes or the graph is stale.
    fn refresh_graph(&mut self) -> bool {
        if !self.state.graph_open {
            return false;
        }
        let dir = self.checkout_dir();
        let g = &mut self.graph;
        let moved = dir != g.dir;
        if moved {
            *g = Graph {
                dir: dir.clone(),
                ..Default::default()
            };
        }
        let stale = g
            .fetched
            .is_none_or(|t| t.elapsed() > Duration::from_secs(5));
        if let Some(dir) = dir
            && stale
            && !g.pending
        {
            g.pending = true;
            g.fetched = Some(Instant::now());
            let tx = self.tx.clone();
            std::thread::spawn(move || {
                let rows = git::graph(&dir, 100).unwrap_or_default();
                let _ = tx.send(AppEvent::Graph(dir, rows));
            });
        }
        moved
    }

    fn toggle_graph(&mut self) {
        self.state.graph_open = !self.state.graph_open;
        let _ = self.state.save();
        self.graph = Graph::default();
        if self.state.graph_open {
            self.refresh_graph();
        } else if self.focus == Focus::Graph {
            self.focus = Focus::List;
        }
        self.toast(if self.state.graph_open {
            "Graph shown"
        } else {
            "Graph hidden"
        });
    }

    fn cycle_sort(&mut self) {
        self.state.list_sort = self.state.list_sort.next();
        let _ = self.state.save();
        self.toast(match self.state.list_sort {
            ListSort::Attention => "Sorted by attention",
            ListSort::NameAsc => "Sorted by name A→Z",
            ListSort::NameDesc => "Sorted by name Z→A",
        });
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
            mode: &self.cfg.default_mode,
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

    /// Sessions running in the worktree at `wt.path`.
    fn worktree_sessions(&self, wt: &git::Worktree) -> Vec<String> {
        let Ok(dir) = wt.path.canonicalize() else {
            return vec![];
        };
        self.sessions
            .iter()
            .filter(|s| s.cwd.canonicalize().is_ok_and(|c| c == dir))
            .map(|s| s.id.clone())
            .collect()
    }

    /// Asks before merging (or discarding) the worktree a session runs in.
    fn request_finish_worktree(&mut self, id: &str, merge: bool) {
        let Some(s) = self.session(id) else { return };
        let Some(wt) = git::linked_worktree(&s.project, &s.cwd) else {
            self.toast("This session isn't in a worktree");
            return;
        };
        let project = s.project.clone();
        let live = self
            .worktree_sessions(&wt)
            .iter()
            .filter(|id| self.session(id).is_some_and(|s| s.pty.is_some()))
            .count();
        let stops = match live {
            0 => String::new(),
            1 => " Stops its session.".into(),
            n => format!(" Stops {n} sessions."),
        };
        let message = if merge {
            if git::changes(&wt.path).is_ok_and(|c| !c.is_empty()) {
                self.toast(format!("{} has uncommitted changes", wt.branch));
                return;
            }
            let base = git::current_branch(&project).unwrap_or_else(|| "main".into());
            format!(
                "Merge {} into {base} and delete the worktree?{stops}",
                wt.branch
            )
        } else {
            format!(
                "Delete worktree {} and its unmerged work?{stops}",
                wt.branch
            )
        };
        self.modal = Some(Modal::Confirm(Confirm {
            message,
            action: ConfirmAction::FinishWorktree { project, wt, merge },
        }));
    }

    /// Merges the worktree's branch (unless discarding), stops and archives its sessions,
    /// then deletes the worktree and branch.
    fn finish_worktree(&mut self, project: &Path, wt: &git::Worktree, merge: bool) {
        let base = if merge {
            match git::merge_worktree(project, wt) {
                Ok(b) => Some(b),
                Err(e) => return self.toast(format!("{e:#}")),
            }
        } else {
            None
        };
        for id in &self.worktree_sessions(wt) {
            self.close_session(id);
        }
        if let Ok(dir) = wt.path.canonicalize() {
            for t in &mut self.terminals {
                if t.cwd.canonicalize().is_ok_and(|c| c == dir) {
                    t.pty.kill();
                }
            }
        }
        match git::remove_worktree(project, wt, !merge) {
            Err(e) => self.toast(format!("Removing the worktree failed: {e:#}")),
            Ok(()) => match base {
                Some(b) => self.toast(format!("Merged {} into {b}", wt.branch)),
                None => self.toast(format!("Discarded {}", wt.branch)),
            },
        }
        self.save();
    }

    pub fn save(&mut self) {
        let mut recs: Vec<_> = self.sessions.iter().map(|s| s.record()).collect();
        recs.sort_by_key(|r| std::cmp::Reverse(r.created));
        recs.truncate(200);
        self.state.sessions = recs;
        let _ = self.state.save();
    }

    pub fn shutdown(&mut self) {
        self.keep_diff_layout();
        self.save();
        for s in &mut self.sessions {
            s.kill();
        }
        for t in &mut self.terminals {
            t.pty.kill();
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
            AppEvent::Committed(root, res) => self.commit_finished(root, res),
            AppEvent::ShellExited(id) => {
                self.terminals.retain(|t| t.id != id);
                if matches!(self.modal, Some(Modal::Terminal(m)) if m == id) {
                    self.close_modal();
                }
            }
            AppEvent::Memory(u) => {
                if let Some(Modal::Memory(m)) = &mut self.modal {
                    *m = Some(u);
                }
            }
            AppEvent::Graph(dir, rows) => {
                let g = &mut self.graph;
                if g.dir.as_ref() == Some(&dir) {
                    // Keep the selected commit selected across refreshes.
                    let keep = g.selected_hash().map(str::to_string);
                    g.rows = rows;
                    g.sel = keep
                        .and_then(|h| {
                            g.rows
                                .iter()
                                .position(|r| r.commit.as_ref().is_some_and(|c| c.hash == h))
                        })
                        .unwrap_or(0);
                    g.step(0);
                    g.loaded = true;
                    g.pending = false;
                    if self.focus == Focus::Graph {
                        self.sync_graph();
                    }
                }
            }
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
        redraw |= self.refresh_graph();
        self.refresh_memory();
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
            Some(Modal::Rename(r)) => r.input.insert_str(s),
            Some(Modal::NewProject(np)) => np.paste(s),
            Some(Modal::Diff(d)) => d.paste(s),
            Some(Modal::Terminal(id)) => {
                if let Some(t) = self.terminals.iter_mut().find(|t| t.id == *id) {
                    let bracketed = t.parser.lock().unwrap().screen().bracketed_paste();
                    t.write(&session::paste_bytes(s, bracketed));
                }
            }
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
                    self.sessions[i].scroll = 0;
                    self.sessions[i].write(&session::paste_bytes(s, bracketed));
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
            detail: "N".into(),
            ..cmd(
                "New project (create a folder)".into(),
                Cmd::NewProject(String::new()),
            )
        });
        items.push(Item {
            detail: k.diff.short(),
            ..cmd("View changes (diff)".into(), Cmd::Diff)
        });
        items.push(Item {
            detail: k.commit.short(),
            ..cmd("Commit changes".into(), Cmd::Commit)
        });
        items.push(Item {
            detail: k.terminal.short(),
            ..cmd("Open terminal".into(), Cmd::Terminal)
        });
        items.push(Item {
            detail: k.memory.short(),
            ..cmd("Memory usage".into(), Cmd::Memory)
        });
        items.push(Item {
            detail: k.settings.short(),
            ..cmd("Settings".into(), Cmd::Settings)
        });
        if let Some(id) = &self.current
            && let Some(s) = self.session(id)
        {
            items.push(cmd(
                format!("Rename session: {}", s.name),
                Cmd::RenameSession(id.clone()),
            ));
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
            if let Some(wt) = git::linked_worktree(&s.project, &s.cwd) {
                items.push(cmd(
                    format!("Merge worktree: {}", wt.branch),
                    Cmd::MergeWorktree(id.clone()),
                ));
                items.push(cmd(
                    format!("Discard worktree: {}", wt.branch),
                    Cmd::DiscardWorktree(id.clone()),
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

    /// Opens the diff viewer on the current session's checkout, in commit mode if `commit`.
    fn open_diff(&mut self, commit: bool) {
        let Some(dir) = self.checkout_dir() else {
            self.toast("Nothing to diff");
            return;
        };
        if !git::is_repo(&dir) {
            self.toast(format!("{} is not a git repository", tilde(&dir)));
            return;
        }
        let mut d = DiffView::open(
            dir,
            self.theme,
            self.state.diff_tree_width,
            self.state.diff_split,
        );
        if commit {
            if !d.has_changes() {
                return self.toast("Nothing to commit");
            }
            d.start_commit();
        }
        self.modal = Some(Modal::Diff(Box::new(d)));
    }

    /// Runs the commit in the background so slow hooks don't freeze the UI.
    fn start_commit(&mut self, root: PathBuf, paths: Vec<String>, message: String) {
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let res = git::commit_paths(&root, &paths, &message).map_err(|e| format!("{e:#}"));
            let _ = tx.send(AppEvent::Committed(root, res));
        });
    }

    fn commit_finished(&mut self, root: PathBuf, res: Result<String, String>) {
        let open = match &mut self.modal {
            Some(Modal::Diff(d)) if d.root == root && d.busy() => Some(d),
            _ => None,
        };
        match (res, open) {
            (Ok(hash), d) => {
                if let Some(d) = d {
                    d.committed();
                    if !d.has_changes() {
                        self.close_modal();
                    }
                }
                self.toast(format!("Committed {hash}"));
                self.graph.fetched = None;
            }
            (Err(e), Some(d)) => d.commit_failed(e),
            (Err(e), None) => self.toast(format!("Commit failed: {e}")),
        }
    }

    /// Opens the new-project modal, creating folders under the first project root.
    fn open_new_project(&mut self, text: &str) {
        let base = self
            .cfg
            .project_roots
            .first()
            .map(|r| expand(r))
            .unwrap_or_else(crate::config::home);
        let np = NewProject::new(base, self.state.new_project_git, text);
        self.modal = Some(Modal::NewProject(np));
    }

    fn project_created(&mut self, path: PathBuf, git_err: Option<String>) {
        self.add_project(path.clone());
        self.scan_folders();
        match git_err {
            Some(e) => self.toast(format!(
                "Created {}, but git init failed: {e}",
                tilde(&path)
            )),
            None => self.toast(format!("Created {}", tilde(&path))),
        }
        self.open_prompt(false);
    }

    fn open_rename(&mut self, id: String) {
        let Some(s) = self.session(&id) else { return };
        let input = TextInput::with_text(&s.name, false);
        self.modal = Some(Modal::Rename(Rename { id, input }));
    }

    /// Shows the shell for the current checkout, starting one if it has none.
    fn open_terminal(&mut self) {
        let Some(dir) = self.checkout_dir() else {
            self.toast("Add a project first");
            return;
        };
        let id = match self.terminals.iter().find(|t| t.cwd == dir) {
            Some(t) => t.id,
            None => {
                self.next_terminal += 1;
                let id = self.next_terminal;
                let (w, h) = crossterm::terminal::size().unwrap_or((120, 40));
                let r = ui::terminal_rect(Rect::new(0, 0, w, h));
                let size = (r.height.saturating_sub(2), r.width.saturating_sub(2));
                let colors = (Theme::rgb_of(self.theme.bg), Theme::rgb_of(self.theme.fg));
                match Terminal::spawn(id, dir, size, colors, self.tx.clone()) {
                    Ok(t) => self.terminals.push(t),
                    Err(e) => return self.toast(format!("Failed to start a shell: {e:#}")),
                }
                id
            }
        };
        self.modal = Some(Modal::Terminal(id));
    }

    fn open_memory(&mut self) {
        self.modal = Some(Modal::Memory(None));
        self.memory_fetched = None;
        self.refresh_memory();
    }

    /// While the memory modal is open, measures again every two seconds in the background.
    fn refresh_memory(&mut self) {
        if !matches!(self.modal, Some(Modal::Memory(_)))
            || self
                .memory_fetched
                .is_some_and(|t| t.elapsed() < Duration::from_secs(2))
        {
            return;
        }
        self.memory_fetched = Some(Instant::now());
        let sessions = self
            .sessions
            .iter()
            .filter_map(|s| {
                let pid = s.pty.as_ref()?.pid?;
                let name = if s.name.is_empty() {
                    "session"
                } else {
                    &s.name
                };
                Some((format!("{} · {name}", self.project_name(&s.project)), pid))
            })
            .collect();
        let shells = self
            .terminals
            .iter()
            .filter_map(|t| Some((tilde(&t.cwd), t.pty.pid?)))
            .collect();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(AppEvent::Memory(memory::measure(sessions, shells)));
        });
    }

    /// Sends a key to the open terminal; shift+PgUp/PgDn scroll it back instead.
    fn terminal_key(&mut self, id: u64, k: KeyEvent) {
        let Some(t) = self.terminals.iter_mut().find(|t| t.id == id) else {
            return;
        };
        let half = (t.pty.size.0 as usize / 2).max(1);
        match k.code {
            KeyCode::PageUp if k.modifiers.contains(KeyModifiers::SHIFT) => t.scroll += half,
            KeyCode::PageDown if k.modifiers.contains(KeyModifiers::SHIFT) => {
                t.scroll = t.scroll.saturating_sub(half)
            }
            _ => {
                let app_cursor = t.parser.lock().unwrap().screen().application_cursor();
                let bytes = keys::encode(&k, app_cursor);
                if !bytes.is_empty() {
                    t.write(&bytes);
                }
            }
        }
    }

    /// Saves the open diff viewer's tree width and split mode when they changed, so they
    /// survive even if maximus is killed with the viewer open.
    fn keep_diff_layout(&mut self) {
        if let Some(Modal::Diff(d)) = &self.modal
            && (self.state.diff_tree_width, self.state.diff_split) != (d.tree_width, d.split)
        {
            self.state.diff_tree_width = d.tree_width;
            self.state.diff_split = d.split;
            let _ = self.state.save();
        }
    }

    fn close_modal(&mut self) {
        self.keep_diff_layout();
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
                Cmd::NewProject(text) => self.open_new_project(&text),
                Cmd::Diff => self.open_diff(false),
                Cmd::Commit => self.open_diff(true),
                Cmd::Terminal => self.open_terminal(),
                Cmd::Memory => self.open_memory(),
                Cmd::Settings => self.modal = Some(Modal::Settings(SettingsModal::new())),
                Cmd::RemoveProject(p) => self.request_remove_project(p),
                Cmd::CloseSession(id) => self.request_close_session(id),
                Cmd::RenameSession(id) => self.open_rename(id),
                Cmd::ResumeSession(id) => {
                    self.resume(&id);
                    self.focus_session(&id);
                }
                Cmd::MergeWorktree(id) => self.request_finish_worktree(&id, true),
                Cmd::DiscardWorktree(id) => self.request_finish_worktree(&id, false),
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
        let plural = |n: usize, what: &str| format!("{n} {what}{}", if n == 1 { "" } else { "s" });
        let running: Vec<String> = [
            (self.live_count(), "session"),
            (self.terminals.len(), "terminal"),
        ]
        .into_iter()
        .filter(|&(n, _)| n > 0)
        .map(|(n, what)| plural(n, what))
        .collect();
        if !running.is_empty() {
            self.modal = Some(Modal::Confirm(Confirm {
                message: format!("{} running. Stop and quit?", running.join(" and ")),
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
                    if d.busy() {
                        // Wait for the commit to finish.
                    } else if self.keys.commit.matches(&k) && !d.committing() {
                        d.start_commit();
                    } else if self.keys.diff.matches(&k) || self.keys.commit.matches(&k) {
                        self.close_modal();
                    } else {
                        match d.handle_key(k) {
                            DiffAction::Close => self.close_modal(),
                            DiffAction::Commit(paths, message) => {
                                let root = d.root.clone();
                                self.start_commit(root, paths, message);
                            }
                            DiffAction::None => self.keep_diff_layout(),
                        }
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
                            ConfirmAction::FinishWorktree { project, wt, merge } => {
                                self.finish_worktree(&project, &wt, merge)
                            }
                        }
                    }
                    KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc | KeyCode::Char('q') => {
                        let _ = c;
                        self.modal = None;
                    }
                    _ => {}
                },
                Modal::Terminal(id) => {
                    let id = *id;
                    if self.keys.terminal.matches(&k) {
                        self.close_modal();
                    } else {
                        self.terminal_key(id, k);
                    }
                }
                Modal::Memory(_) => {
                    if self.keys.memory.matches(&k)
                        || matches!(k.code, KeyCode::Esc | KeyCode::Char('q'))
                    {
                        self.close_modal();
                    }
                }
                Modal::NewProject(np) => match np.handle_key(k) {
                    NewProjectAction::None => {}
                    NewProjectAction::Close => {
                        self.state.new_project_git = np.git;
                        let _ = self.state.save();
                        self.modal = None;
                    }
                    NewProjectAction::Created(path, git_err) => {
                        self.state.new_project_git = np.git;
                        self.modal = None;
                        self.project_created(path, git_err);
                    }
                },
                Modal::Rename(r) => match k.code {
                    KeyCode::Enter => {
                        let name = r.input.text.trim().to_string();
                        let id = r.id.clone();
                        self.modal = None;
                        if !name.is_empty()
                            && let Some(i) = self.session_idx(&id)
                        {
                            self.sessions[i].name = name;
                            self.save();
                        }
                    }
                    KeyCode::Esc => self.modal = None,
                    _ => {
                        r.input.handle_key(&k);
                    }
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
            return self.open_diff(false);
        }
        if g.commit.matches(&k) {
            return self.open_diff(true);
        }
        if g.terminal.matches(&k) {
            return self.open_terminal();
        }
        if g.memory.matches(&k) {
            return self.open_memory();
        }
        if g.settings.matches(&k) {
            self.modal = Some(Modal::Settings(SettingsModal::new()));
            return;
        }
        // In the left column, alt+down/up step between the Agents and Graph panels.
        if g.next_session.matches(&k) && self.focus == Focus::List && self.state.graph_open {
            self.focus = Focus::Graph;
            self.sync_graph();
            return;
        }
        if g.prev_session.matches(&k) && self.focus == Focus::Graph {
            self.focus = Focus::List;
            return;
        }
        if g.next_session.matches(&k) && self.focus == Focus::Graph {
            return;
        }
        if g.next_session.matches(&k) {
            return self.cycle_attention(1);
        }
        if g.prev_session.matches(&k) {
            return self.cycle_attention(-1);
        }
        if g.graph.matches(&k) {
            return self.toggle_graph();
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
            Focus::Graph => {
                self.graph_key(k);
                self.sync_graph();
            }
        }
    }

    fn sync_graph(&mut self) {
        self.graph.sync(self.theme, self.state.diff_split);
    }

    fn graph_key(&mut self, k: KeyEvent) {
        let g = &mut self.graph;
        let page = (self.pane_inner.height as i32 / 2).max(1);
        match k.code {
            KeyCode::Down | KeyCode::Char('j') => g.down(),
            KeyCode::Up | KeyCode::Char('k') => g.up(),
            KeyCode::Char('g') | KeyCode::Home => g.step(-10_000),
            KeyCode::Char('G') | KeyCode::End => g.step(10_000),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right if g.file.is_none() => {
                if g.collapsed {
                    g.collapsed = false;
                } else {
                    g.down();
                }
            }
            KeyCode::PageDown | KeyCode::Char(' ') => {
                if let Some(p) = &mut g.preview {
                    p.scroll_by(page);
                }
            }
            KeyCode::PageUp => {
                if let Some(p) = &mut g.preview {
                    p.scroll_by(-page);
                }
            }
            KeyCode::Char('l') | KeyCode::Right => {
                if let Some(p) = &mut g.preview {
                    p.pan_by(8);
                }
            }
            KeyCode::Char('h') | KeyCode::Left => {
                if let Some(p) = &mut g.preview {
                    p.pan_by(-8);
                }
            }
            KeyCode::Char('s') => {
                self.state.diff_split = !self.state.diff_split;
                let _ = self.state.save();
                if let Some(p) = &mut self.graph.preview {
                    p.split = self.state.diff_split;
                }
            }
            KeyCode::Char('y') => {
                if let Some(h) = g.selected_hash().map(str::to_string) {
                    notify::copy(&h);
                    self.toast(format!("Copied {h}"));
                }
            }
            KeyCode::Esc => {
                if g.expanded() {
                    g.collapsed = true;
                    g.file = None;
                } else {
                    self.focus = Focus::List;
                }
            }
            KeyCode::Char('t') => self.toggle_graph(),
            KeyCode::Char('d') => self.open_diff(false),
            KeyCode::Char('n') => self.open_prompt(false),
            KeyCode::Char('/') => self.open_switcher(),
            KeyCode::Char(',') => self.modal = Some(Modal::Settings(SettingsModal::new())),
            KeyCode::Char('q') => self.request_quit(),
            KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                self.request_quit()
            }
            _ => {}
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
            KeyCode::Char('N') => self.open_new_project(""),
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
            KeyCode::Char('e') => {
                if let Some(RowKey::Session(id)) = self.selected.clone() {
                    self.open_rename(id);
                }
            }
            KeyCode::Char('m') => {
                if let Some(RowKey::Session(id)) = self.selected.clone() {
                    self.request_finish_worktree(&id, true);
                }
            }
            KeyCode::Char('d') => self.open_diff(false),
            KeyCode::Char('t') => self.toggle_graph(),
            KeyCode::Char('s') => self.cycle_sort(),
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
                Modal::Diff(d) => {
                    d.handle_mouse(m);
                    // Save once a separator drag ends, not on every step.
                    if matches!(m.kind, MouseEventKind::Up(_)) {
                        self.keep_diff_layout();
                    }
                }
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
                Modal::Terminal(id) => {
                    let up = match m.kind {
                        MouseEventKind::ScrollUp => true,
                        MouseEventKind::ScrollDown => false,
                        _ => return,
                    };
                    let Some(t) = self.terminals.iter_mut().find(|t| t.id == *id) else {
                        return;
                    };
                    // Full-screen programs get arrow keys; a plain shell scrolls back.
                    if t.parser.lock().unwrap().screen().alternate_screen() {
                        let seq: &[u8] = if up { b"\x1b[A" } else { b"\x1b[B" };
                        t.write(&seq.repeat(3));
                    } else if up {
                        t.scroll += 3;
                    } else {
                        t.scroll = t.scroll.saturating_sub(3);
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
                } else if self.graph_rect.height > 0
                    && x >= self.list_rect.x
                    && x < sep
                    && (y == self.graph_rect.y || y + 1 == self.graph_rect.y)
                {
                    // The Agents bottom border or the Graph top border.
                    self.drag = Drag::GraphSeparator;
                } else if inside(self.graph_rect) {
                    self.focus = Focus::Graph;
                    let g = &mut self.graph;
                    let i = g.offset + y.saturating_sub(self.graph_rect.y + 1) as usize;
                    match g.lines().get(i) {
                        Some(GraphLine::Row(r)) if g.rows[*r].commit.is_some() => {
                            if *r != g.sel {
                                g.sel = *r;
                                g.collapsed = false;
                            }
                            g.file = None;
                        }
                        Some(GraphLine::File(f)) => g.file = Some(*f),
                        _ => {}
                    }
                    self.sync_graph();
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
                Drag::GraphSeparator => {
                    let bottom = self.list_rect.y + self.list_rect.height;
                    self.state.graph_height =
                        ui::graph_height(bottom.saturating_sub(y), self.list_rect.height);
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
                    Drag::Separator | Drag::GraphSeparator => {
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
                if inside(self.graph_rect) {
                    if up {
                        self.graph.up();
                    } else {
                        self.graph.down();
                    }
                    self.sync_graph();
                } else if self.focus == Focus::Graph
                    && inside(self.pane_rect)
                    && let Some(p) = &mut self.graph.preview
                {
                    p.scroll_by(if up { -3 } else { 3 });
                } else if inside(self.list_rect) {
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

#[cfg(test)]
mod tests {
    use super::natural_cmp;
    use std::cmp::Ordering::*;

    #[test]
    fn natural_order() {
        assert_eq!(natural_cmp("agent 2", "agent 10"), Less);
        assert_eq!(natural_cmp("Beta", "alpha"), Greater);
        assert_eq!(natural_cmp("api", "API"), Equal);
        assert_eq!(natural_cmp("v007", "v7"), Equal);
        assert_eq!(natural_cmp("fix", "fix keys"), Less);
        assert_eq!(natural_cmp("a1b", "a1a"), Greater);
    }
}
