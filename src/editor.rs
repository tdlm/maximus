use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
};
use two_face::re_exports::syntect::{
    easy::HighlightLines,
    highlighting::{HighlightState, Theme as SynTheme},
    parsing::{ParseState, SyntaxReference},
};

use crate::{
    app::GlobalKeys,
    buffer::{self, Buffer, Pos},
    config::tilde,
    diff::{self, HlLine},
    git, notify,
    textinput::TextInput,
    theme::Theme,
    ui,
};

/// How often the file list is read again, so files claude creates show up.
const LIST_EVERY: Duration = Duration::from_secs(3);
/// How often the open file is checked for changes on disk.
const CHECK_EVERY: Duration = Duration::from_secs(1);
/// Most files listed from a folder that isn't a git repository.
const MAX_WALK: usize = 50_000;
/// Most matches the file filter shows.
const MAX_MATCHES: usize = 500;

/// The files under `root`: what git tracks or would track (so ignored files stay out), or a
/// plain walk skipping hidden folders, `node_modules` and `target` outside a repository.
pub fn list_files(root: &Path) -> Vec<String> {
    let mut files: Vec<String> = match git::files(root) {
        Ok(f) => f.into_iter().filter(|p| root.join(p).is_file()).collect(),
        Err(_) => walk(root),
    };
    files.sort();
    files.dedup();
    files
}

fn walk(root: &Path) -> Vec<String> {
    let mut out = vec![];
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        let Ok(rd) = fs::read_dir(root.join(&rel)) else {
            continue;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name == "node_modules" || name == "target" {
                continue;
            }
            let path = rel.join(&name);
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(path),
                Ok(_) => out.push(path.to_string_lossy().into_owned()),
                Err(_) => {}
            }
            if out.len() >= MAX_WALK {
                return out;
            }
        }
    }
    out
}

fn mtime_of(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Focus {
    Tree,
    File,
    Claude,
}

pub enum EditorAction {
    None,
    Close,
}

/// Whether a mouse event was the editor's or belongs to the claude panel.
pub enum MouseTarget {
    Editor,
    Claude,
    /// A click in the save-changes dialog closed the editor.
    Close,
}

/// What to do once the save-changes prompt is answered.
enum Pending {
    /// Close the editor, saving or discarding every tab's edits.
    Close,
    /// Close this file's tab.
    CloseTab(String),
}

/// The save-changes dialog's buttons.
#[derive(Clone, Copy, PartialEq)]
enum Answer {
    Save,
    Discard,
    Cancel,
}

#[derive(Clone, Copy, PartialEq)]
enum Drag {
    TreeSeparator,
    ClaudeSeparator,
    Select,
}

#[derive(Default)]
struct Node {
    dirs: BTreeMap<String, Node>,
    files: Vec<String>,
}

/// Where a tab was drawn on the tab row: columns `x..end`, with its close mark at `close`.
struct TabHit {
    tab: usize,
    x: u16,
    end: u16,
    close: u16,
}

struct TreeRow {
    depth: usize,
    label: String,
    /// The file's path, or the directory's path with a trailing slash.
    key: String,
    dir: bool,
    open: bool,
}

/// Highlights a buffer's lines as far as they're drawn, keeping the parser state before each
/// line so an edit only re-highlights from the line it changed.
struct Highlighter {
    theme: SynTheme,
    syntax: &'static SyntaxReference,
    lines: Vec<HlLine>,
    /// `states[i]` is the state before line `i`.
    states: Vec<(HighlightState, ParseState)>,
    /// Huge files skip highlighting, like the diff viewer.
    plain: bool,
}

impl Highlighter {
    fn new(path: &str, theme: &Theme, lines: usize) -> Self {
        let theme = diff::syn_theme(theme.syntax);
        let syntax = diff::syntax_for(path);
        let start = HighlightLines::new(syntax, &theme).state();
        Self {
            theme,
            syntax,
            lines: vec![],
            states: vec![start],
            plain: lines > 30_000,
        }
    }

    fn invalidate(&mut self, row: usize) {
        self.lines.truncate(row);
        self.states.truncate(row + 1);
    }

    fn ensure(&mut self, text: &[String], upto: usize) {
        let upto = upto.min(text.len());
        while self.lines.len() < upto {
            let i = self.lines.len();
            if self.plain {
                self.lines.push(vec![(Style::default(), text[i].clone())]);
                continue;
            }
            let (hs, ps) = self.states[i].clone();
            let mut h = HighlightLines::from_state(&self.theme, hs, ps);
            // The syntaxes expect each line to end in a newline.
            let line = format!("{}\n", text[i]);
            let spans = match h.highlight_line(&line, diff::syntaxes()) {
                Ok(r) => r
                    .into_iter()
                    .map(|(st, s)| (diff::style_of(st), s.trim_end_matches('\n').to_string()))
                    .filter(|(_, s)| !s.is_empty())
                    .collect(),
                Err(_) => vec![(Style::default(), text[i].clone())],
            };
            self.lines.push(spans);
            self.states.push(h.state());
        }
    }

    /// Starts over in another theme's colours.
    fn rebuild(&mut self, theme: &Theme) {
        self.theme = diff::syn_theme(theme.syntax);
        self.lines.clear();
        self.states = vec![HighlightLines::new(self.syntax, &self.theme).state()];
    }
}

struct OpenFile {
    /// Relative to the editor's root.
    path: String,
    /// None when the file can't be edited as text; `note` says why.
    buf: Option<Buffer>,
    note: Option<&'static str>,
    mtime: Option<SystemTime>,
    /// Changed on disk while it had unsaved edits.
    stale: bool,
    /// First visible line and column.
    top: usize,
    left: usize,
    /// Scroll to the cursor on the next draw.
    follow: bool,
    hl: Option<Highlighter>,
    /// Shown from the list with space: the next file shown that way takes its tab, unless it
    /// was edited or opened for real meanwhile.
    preview: bool,
}

impl OpenFile {
    fn dirty(&self) -> bool {
        self.buf.as_ref().is_some_and(|b| b.dirty())
    }
}

fn load(root: &Path, path: &str, theme: &Theme) -> Result<OpenFile, String> {
    let full = root.join(path);
    let bytes = fs::read(&full).map_err(|e| format!("Couldn't open {path}: {e}"))?;
    let mtime = mtime_of(&full);
    let bytes = Some(bytes);
    let mut note = diff::unreadable(&None, &bytes);
    let mut text = None;
    if note.is_none()
        && let Some(bytes) = bytes
    {
        match String::from_utf8(bytes) {
            Ok(t) => text = Some(t),
            Err(_) => note = Some("Not UTF-8 text"),
        }
    }
    let buf = text.map(|t| Buffer::new(&t));
    let hl = buf
        .as_ref()
        .map(|b| Highlighter::new(path, theme, b.lines.len()));
    Ok(OpenFile {
        path: path.to_string(),
        buf,
        note,
        mtime,
        stale: false,
        top: 0,
        left: 0,
        follow: true,
        hl,
        preview: false,
    })
}

struct Find {
    input: TextInput,
    /// Where the cursor was when find opened; typing searches from here.
    origin: Pos,
    miss: bool,
}

/// The editor view: a checkout's files on the left, the open file in the middle, and the
/// session's claude on the right (drawn by the ui, since it needs the session).
pub struct Editor {
    pub root: PathBuf,
    /// The session whose claude shows on the right.
    pub session: Option<String>,
    pub focus: Focus,
    theme_id: &'static str,
    files: Vec<String>,
    changes: HashMap<String, char>,
    /// Directories (with a trailing slash) holding changed files.
    changed_dirs: HashSet<String>,
    tree: Node,
    loaded: bool,
    expanded: HashSet<String>,
    rows: Vec<TreeRow>,
    sel: usize,
    tree_offset: usize,
    filter: Option<TextInput>,
    /// Open files, one tab each, and the one shown.
    tabs: Vec<OpenFile>,
    active: usize,
    /// First tab drawn, when they don't all fit.
    tab_offset: usize,
    find: Option<Find>,
    pending: Option<Pending>,
    /// The last text copied or cut, for ctrl+v.
    clip: String,
    /// A status line message, and whether it's an error.
    message: Option<(String, bool, Instant)>,
    pub tree_width: u16,
    /// 0 sizes the claude panel automatically.
    pub claude_width: u16,
    tree_rect: Rect,
    tree_inner: Rect,
    file_rect: Rect,
    /// Each drawn tab's columns on the tab row, and where its close mark is.
    tab_hits: Vec<TabHit>,
    tab_row: Rect,
    /// Where the save-changes dialog drew its buttons.
    buttons: Vec<(Rect, Answer)>,
    text_rect: Rect,
    claude_rect: Rect,
    drag: Option<Drag>,
    fetching: bool,
    fetched: Option<Instant>,
    checked: Instant,
}

impl Editor {
    pub fn new(
        root: PathBuf,
        session: Option<String>,
        theme: &'static Theme,
        tree_width: u16,
        claude_width: u16,
    ) -> Self {
        Self {
            root,
            session,
            focus: Focus::Tree,
            theme_id: theme.id,
            files: vec![],
            changes: HashMap::new(),
            changed_dirs: HashSet::new(),
            tree: Node::default(),
            loaded: false,
            expanded: HashSet::new(),
            rows: vec![],
            sel: 0,
            tree_offset: 0,
            filter: None,
            tabs: vec![],
            active: 0,
            tab_offset: 0,
            find: None,
            pending: None,
            clip: String::new(),
            message: None,
            tree_width,
            claude_width,
            tree_rect: Rect::default(),
            tree_inner: Rect::default(),
            file_rect: Rect::default(),
            tab_hits: vec![],
            tab_row: Rect::default(),
            buttons: vec![],
            text_rect: Rect::default(),
            claude_rect: Rect::default(),
            drag: None,
            fetching: false,
            fetched: None,
            checked: Instant::now(),
        }
    }

    /// Picks up where it was left: the same session or another in the checkout, the current
    /// theme, and whatever changed on disk meanwhile.
    pub fn reopen(&mut self, session: Option<String>, theme: &'static Theme) {
        self.session = session;
        if theme.id != self.theme_id {
            self.theme_id = theme.id;
            for hl in self.tabs.iter_mut().filter_map(|f| f.hl.as_mut()) {
                hl.rebuild(theme);
            }
        }
        self.fetched = None;
        self.check_disk(theme);
    }

    pub fn claude_shown(&self) -> bool {
        self.claude_rect.width > 0
    }

    pub fn has_prompt(&self) -> bool {
        self.pending.is_some()
    }

    /// The file shown.
    fn file(&self) -> Option<&OpenFile> {
        self.tabs.get(self.active)
    }

    fn tab_of(&self, path: &str) -> Option<usize> {
        self.tabs.iter().position(|f| f.path == path)
    }

    fn dirty_count(&self) -> usize {
        self.tabs.iter().filter(|f| f.dirty()).count()
    }

    fn say(&mut self, msg: impl Into<String>, error: bool) {
        self.message = Some((msg.into(), error, Instant::now()));
    }

    // ---------- files ----------

    /// Whether it's time to read the file list again.
    pub fn wants_files(&self) -> bool {
        !self.fetching && self.fetched.is_none_or(|t| t.elapsed() >= LIST_EVERY)
    }

    pub fn files_requested(&mut self) {
        self.fetching = true;
        self.fetched = Some(Instant::now());
    }

    pub fn set_files(&mut self, files: Vec<String>, changes: Vec<git::Change>) {
        self.fetching = false;
        let changes: HashMap<String, char> =
            changes.into_iter().map(|c| (c.path, c.status)).collect();
        if self.loaded && files == self.files && changes == self.changes {
            return;
        }
        self.loaded = true;
        self.changed_dirs.clear();
        for path in changes.keys() {
            for (i, _) in path.match_indices('/') {
                self.changed_dirs.insert(path[..=i].to_string());
            }
        }
        let mut tree = Node::default();
        for f in &files {
            let parts: Vec<&str> = f.split('/').collect();
            let mut n = &mut tree;
            for d in &parts[..parts.len() - 1] {
                n = n.dirs.entry(d.to_string()).or_default();
            }
            n.files.push(parts[parts.len() - 1].to_string());
        }
        self.tree = tree;
        self.files = files;
        self.changes = changes;
        self.rebuild_rows();
    }

    /// Lays out the tree's rows (or the filter's matches), keeping the selected row.
    fn rebuild_rows(&mut self) {
        let keep = self.rows.get(self.sel).map(|r| r.key.clone());
        self.rows.clear();
        if let Some(input) = &self.filter {
            let q = input.text.trim();
            if q.is_empty() {
                self.rows = self
                    .files
                    .iter()
                    .take(MAX_MATCHES)
                    .map(|f| file_row(f))
                    .collect();
            } else {
                let m = SkimMatcherV2::default().ignore_case();
                let mut hits: Vec<(i64, &String)> = self
                    .files
                    .iter()
                    .filter_map(|f| m.fuzzy_match(f, q).map(|s| (s, f)))
                    .collect();
                hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.len().cmp(&b.1.len())));
                self.rows = hits
                    .into_iter()
                    .take(MAX_MATCHES)
                    .map(|(_, f)| file_row(f))
                    .collect();
                // The best match first.
                self.sel = 0;
                return;
            }
        } else {
            walk_tree(&self.tree, "", 0, &self.expanded, &mut self.rows);
        }
        self.sel = keep
            .and_then(|k| self.rows.iter().position(|r| r.key == k))
            .unwrap_or(self.sel.min(self.rows.len().saturating_sub(1)));
    }

    /// Opens the folders down to `path` and selects it.
    fn reveal(&mut self, path: &str) {
        for (i, _) in path.match_indices('/') {
            self.expanded.insert(path[..=i].to_string());
        }
        self.rebuild_rows();
        if let Some(i) = self.rows.iter().position(|r| r.key == path) {
            self.sel = i;
        }
    }

    fn toggle_dir(&mut self, idx: usize) {
        let key = self.rows[idx].key.clone();
        if !self.expanded.remove(&key) {
            self.expanded.insert(key.clone());
        }
        self.rebuild_rows();
        self.sel = self.rows.iter().position(|r| r.key == key).unwrap_or(0);
    }

    /// Shows `path` in its tab, opening one beside the current tab if it isn't open. With
    /// `preview` the file takes the clean preview tab's place instead, and stays a preview.
    fn open_path(&mut self, path: String, focus: bool, preview: bool, theme: &Theme) {
        if focus {
            self.focus = Focus::File;
        }
        if let Some(i) = self.tab_of(&path) {
            if !preview {
                self.tabs[i].preview = false;
            }
            self.activate(i);
            return;
        }
        let mut of = match load(&self.root, &path, theme) {
            Ok(of) => of,
            Err(e) => {
                self.focus = Focus::Tree;
                return self.say(e, true);
            }
        };
        of.preview = preview;
        let at = match self.tabs.iter().position(|f| f.preview && !f.dirty()) {
            Some(i) if preview => {
                self.tabs[i] = of;
                i
            }
            _ => {
                let at = if self.tabs.is_empty() {
                    0
                } else {
                    self.active + 1
                };
                self.tabs.insert(at, of);
                at
            }
        };
        self.activate(at);
    }

    /// Shows tab `i`, selecting its file in the list if it's there.
    fn activate(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        if i != self.active {
            self.find = None;
        }
        self.active = i;
        let path = &self.tabs[i].path;
        if self.filter.is_none()
            && let Some(row) = self.rows.iter().position(|r| &r.key == path)
        {
            self.sel = row;
        }
    }

    /// Moves `delta` tabs along, wrapping around.
    fn cycle_tab(&mut self, delta: isize) {
        let n = self.tabs.len() as isize;
        if n > 0 {
            self.activate((self.active as isize + delta).rem_euclid(n) as usize);
        }
    }

    /// Closes tab `i`, asking first if it has unsaved changes.
    fn close_tab(&mut self, i: usize) {
        let Some(of) = self.tabs.get(i) else { return };
        if of.dirty() {
            self.pending = Some(Pending::CloseTab(of.path.clone()));
            self.activate(i);
            return;
        }
        self.remove_tab(i);
    }

    /// Drops tab `i`, showing the tab to its right (else its left) if it was the one shown.
    fn remove_tab(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        self.tabs.remove(i);
        if i < self.active || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
        if i == self.active || self.tabs.is_empty() {
            self.find = None;
        }
        if self.tabs.is_empty() && self.focus == Focus::File {
            self.focus = Focus::Tree;
        }
    }

    /// Reads tab `i`'s file again, keeping the cursor and scroll where they were.
    fn reload_file(&mut self, i: usize, theme: &Theme) {
        let Some(old) = self.tabs.get_mut(i) else {
            return;
        };
        match load(&self.root, &old.path, theme) {
            Ok(mut of) => {
                if let (Some(ob), Some(nb)) = (&old.buf, &mut of.buf) {
                    nb.move_to(ob.cur, false);
                }
                of.top = old.top;
                of.left = old.left;
                of.follow = false;
                of.preview = old.preview;
                *old = of;
            }
            // Gone or unreadable: keep what's shown, flagged.
            Err(_) => {
                old.stale = true;
                old.mtime = None;
            }
        }
    }

    /// Reloads each open file that changed on disk and has no unsaved edits, and flags those
    /// that do. Returns true if anything changed.
    fn check_disk(&mut self, theme: &Theme) -> bool {
        let mut changed = false;
        for i in 0..self.tabs.len() {
            let of = &mut self.tabs[i];
            let now = mtime_of(&self.root.join(&of.path));
            if now == of.mtime {
                continue;
            }
            changed = true;
            if of.dirty() {
                of.mtime = now;
                of.stale = true;
            } else {
                self.reload_file(i, theme);
            }
        }
        changed
    }

    /// Called ~10x/second. Returns true if a redraw is needed.
    pub fn tick(&mut self, theme: &Theme) -> bool {
        let mut redraw = false;
        if self
            .message
            .as_ref()
            .is_some_and(|m| m.2.elapsed() > Duration::from_secs(4))
        {
            self.message = None;
            redraw = true;
        }
        if self.checked.elapsed() >= CHECK_EVERY {
            self.checked = Instant::now();
            redraw |= self.check_disk(theme);
        }
        redraw
    }

    /// Saves tab `i`, saying how it went.
    fn save(&mut self, i: usize) -> bool {
        let Some(of) = self.tabs.get_mut(i) else {
            return false;
        };
        let Some(buf) = &mut of.buf else {
            return false;
        };
        let full = self.root.join(&of.path);
        let res = match fs::write(&full, buf.text()) {
            Ok(()) => {
                buf.mark_saved();
                of.mtime = mtime_of(&full);
                of.stale = false;
                of.preview = false;
                Ok(format!("Saved {}", of.path))
            }
            Err(e) => Err(format!("Couldn't save {}: {e}", of.path)),
        };
        match res {
            Ok(msg) => {
                self.say(msg, false);
                // Pick up the file's new git status.
                self.fetched = None;
                true
            }
            Err(msg) => {
                self.say(msg, true);
                false
            }
        }
    }

    /// Saves every tab with unsaved changes; false if any couldn't be saved.
    fn save_all(&mut self) -> bool {
        let dirty: Vec<usize> = (0..self.tabs.len())
            .filter(|&i| self.tabs[i].dirty())
            .collect();
        let saved = dirty.iter().filter(|&&i| self.save(i)).count();
        if saved > 1 && saved == dirty.len() {
            self.say(format!("Saved {saved} files"), false);
        }
        saved == dirty.len()
    }

    /// Closes now unless there are unsaved changes, in which case it asks first.
    pub fn request_close(&mut self) -> bool {
        if self.dirty_count() > 0 {
            self.pending = Some(Pending::Close);
            false
        } else {
            true
        }
    }

    // ---------- focus ----------

    /// The key that focuses the agent list moves one panel left.
    pub fn focus_left(&mut self) {
        self.focus = match self.focus {
            Focus::Claude => Focus::File,
            _ => Focus::Tree,
        };
    }

    /// The key that focuses the pane moves one panel right; the app moves on from the file to
    /// claude, since that can start a session.
    pub fn focus_right(&mut self) {
        if self.focus == Focus::Tree {
            self.focus = Focus::File;
        }
    }

    // ---------- keys ----------

    pub fn paste(&mut self, s: &str) {
        match self.focus {
            Focus::Tree => {
                if let Some(input) = &mut self.filter {
                    input.insert_str(s);
                    self.rebuild_rows();
                }
            }
            Focus::File => {
                if let Some(find) = &mut self.find {
                    find.input.insert_str(s);
                } else if let Some(of) = self.tabs.get_mut(self.active)
                    && let Some(buf) = &mut of.buf
                {
                    buf.insert(s);
                    of.follow = true;
                    of.preview = false;
                }
            }
            Focus::Claude => {}
        }
    }

    pub fn handle_key(&mut self, k: KeyEvent, theme: &Theme) -> EditorAction {
        if let Some(p) = self.pending.take() {
            return self.prompt_key(k, p, theme);
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        // The filter and find lines keep ctrl+w for deleting a word.
        let typing = match self.focus {
            Focus::Tree => self.filter.is_some(),
            Focus::File => self.find.is_some(),
            Focus::Claude => false,
        };
        match k.code {
            KeyCode::Char('s') if ctrl => {
                self.save(self.active);
                return EditorAction::None;
            }
            KeyCode::Char('w') if ctrl && !typing => {
                self.close_tab(self.active);
                return EditorAction::None;
            }
            KeyCode::PageUp if ctrl => {
                self.cycle_tab(-1);
                return EditorAction::None;
            }
            KeyCode::PageDown if ctrl => {
                self.cycle_tab(1);
                return EditorAction::None;
            }
            KeyCode::Char(c @ '1'..='9') if alt => {
                self.activate(c as usize - '1' as usize);
                return EditorAction::None;
            }
            _ => {}
        }
        match self.focus {
            Focus::Tree => return self.tree_key(k, theme),
            Focus::File if self.find.is_some() => self.find_key(k),
            Focus::File => self.file_key(k),
            Focus::Claude => {}
        }
        EditorAction::None
    }

    fn prompt_key(&mut self, k: KeyEvent, p: Pending, theme: &Theme) -> EditorAction {
        let answer = match k.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => Answer::Save,
            KeyCode::Char('n') | KeyCode::Char('N') => Answer::Discard,
            KeyCode::Esc => Answer::Cancel,
            _ => {
                self.pending = Some(p);
                return EditorAction::None;
            }
        };
        self.answer(p, answer, theme)
    }

    /// Carries out the save-changes prompt's answer.
    fn answer(&mut self, p: Pending, answer: Answer, theme: &Theme) -> EditorAction {
        match answer {
            Answer::Cancel => return EditorAction::None,
            Answer::Save => {
                let saved = match &p {
                    Pending::Close => self.save_all(),
                    Pending::CloseTab(path) => self.tab_of(path).is_some_and(|i| self.save(i)),
                };
                if !saved {
                    return EditorAction::None;
                }
            }
            Answer::Discard => {}
        }
        match p {
            Pending::Close => {
                // Drop the edits so the editor comes back clean next time.
                if answer == Answer::Discard {
                    for i in 0..self.tabs.len() {
                        if self.tabs[i].dirty() {
                            self.reload_file(i, theme);
                        }
                    }
                }
                EditorAction::Close
            }
            Pending::CloseTab(path) => {
                if let Some(i) = self.tab_of(&path) {
                    self.remove_tab(i);
                }
                EditorAction::None
            }
        }
    }

    fn tree_key(&mut self, k: KeyEvent, theme: &Theme) -> EditorAction {
        let page = self.tree_inner.height.saturating_sub(1).max(1) as usize;
        let last = self.rows.len().saturating_sub(1);
        if let Some(input) = &mut self.filter {
            match k.code {
                KeyCode::Esc => {
                    self.filter = None;
                    self.rebuild_rows();
                }
                KeyCode::Enter => {
                    if let Some(path) = self.rows.get(self.sel).map(|r| r.key.clone()) {
                        self.filter = None;
                        self.reveal(&path);
                        self.open_path(path, true, false, theme);
                    }
                }
                KeyCode::Down => self.sel = (self.sel + 1).min(last),
                KeyCode::Up => self.sel = self.sel.saturating_sub(1),
                KeyCode::PageDown => self.sel = (self.sel + page).min(last),
                KeyCode::PageUp => self.sel = self.sel.saturating_sub(page),
                _ => {
                    let before = input.text.clone();
                    input.handle_key(&k);
                    if input.text != before {
                        self.rebuild_rows();
                    }
                }
            }
            return EditorAction::None;
        }
        match k.code {
            KeyCode::Down | KeyCode::Char('j') => self.sel = (self.sel + 1).min(last),
            KeyCode::Up | KeyCode::Char('k') => self.sel = self.sel.saturating_sub(1),
            KeyCode::PageDown => self.sel = (self.sel + page).min(last),
            KeyCode::PageUp => self.sel = self.sel.saturating_sub(page),
            KeyCode::Char('g') | KeyCode::Home => self.sel = 0,
            KeyCode::Char('G') | KeyCode::End => self.sel = last,
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right | KeyCode::Char(' ') => {
                if let Some(r) = self.rows.get(self.sel) {
                    if r.dir {
                        if !(r.open && k.code == KeyCode::Right) {
                            self.toggle_dir(self.sel);
                        }
                    } else {
                        // Space previews the file but stays in the tree.
                        let path = r.key.clone();
                        let space = k.code == KeyCode::Char(' ');
                        self.open_path(path, !space, space, theme);
                    }
                }
            }
            KeyCode::Char('h') | KeyCode::Left => {
                if let Some(r) = self.rows.get(self.sel) {
                    if r.dir && r.open {
                        self.toggle_dir(self.sel);
                    } else if let Some(i) = (0..self.sel)
                        .rev()
                        .find(|&i| self.rows[i].dir && self.rows[i].depth < r.depth)
                    {
                        self.sel = i;
                    }
                }
            }
            KeyCode::Tab if !self.tabs.is_empty() => self.focus = Focus::File,
            KeyCode::Char('/') => {
                self.filter = Some(TextInput::new(false));
                self.rebuild_rows();
            }
            KeyCode::Char('r') => self.fetched = None,
            // Asks first if the file has unsaved changes.
            KeyCode::Esc | KeyCode::Char('q') if self.request_close() => {
                return EditorAction::Close;
            }
            _ => {}
        }
        EditorAction::None
    }

    fn file_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let page = self.text_rect.height.saturating_sub(1).max(1) as isize;
        let Some(of) = self.tabs.get_mut(self.active) else {
            if k.code == KeyCode::Esc {
                self.focus = Focus::Tree;
            }
            return;
        };
        let Some(buf) = &mut of.buf else {
            if k.code == KeyCode::Esc {
                self.focus = Focus::Tree;
            }
            return;
        };
        of.follow = true;
        let mut copied = None;
        match k.code {
            KeyCode::Esc if buf.anchor.is_some() => buf.anchor = None,
            KeyCode::Esc => self.focus = Focus::Tree,
            KeyCode::Char('z') if ctrl => {
                buf.undo();
            }
            KeyCode::Char('y') if ctrl => {
                buf.redo();
            }
            KeyCode::Char('a') if ctrl => buf.select_all(),
            KeyCode::Char('c') | KeyCode::Char('x') if ctrl => {
                // With nothing selected, the whole line.
                if buf.selection().is_none() {
                    let row = buf.cur.row;
                    buf.move_to(Pos::new(row, 0), false);
                    if row + 1 < buf.lines.len() {
                        buf.move_to(Pos::new(row + 1, 0), true);
                    } else {
                        buf.end(true);
                    }
                }
                copied = if k.code == KeyCode::Char('x') {
                    buf.cut()
                } else {
                    let text = buf.selected_text();
                    buf.anchor = None;
                    text
                };
            }
            KeyCode::Char('v') if ctrl => {
                if !self.clip.is_empty() {
                    buf.insert(&self.clip);
                }
            }
            KeyCode::Char('f') if ctrl => {
                // Start from the selection, so ctrl+f on a word searches for it.
                let input = match buf.selected_text().filter(|s| !s.contains('\n')) {
                    Some(s) => TextInput::with_text(&s, false),
                    None => TextInput::new(false),
                };
                let origin = buf.selection().map_or(buf.cur, |(a, _)| a);
                self.find = Some(Find {
                    input,
                    origin,
                    miss: false,
                });
            }
            KeyCode::Char('b') if alt => buf.word_left(shift),
            KeyCode::Char('f') if alt => buf.word_right(shift),
            KeyCode::Char(c) if !ctrl && !alt => buf.insert(&c.to_string()),
            KeyCode::Enter => buf.newline(),
            KeyCode::Backspace if alt || ctrl => buf.delete_word_back(),
            KeyCode::Backspace => buf.backspace(),
            KeyCode::Delete => buf.delete(),
            KeyCode::BackTab => buf.dedent(),
            KeyCode::Tab if shift => buf.dedent(),
            KeyCode::Tab => buf.indent(),
            KeyCode::Left if alt || ctrl => buf.word_left(shift),
            KeyCode::Right if alt || ctrl => buf.word_right(shift),
            KeyCode::Left => buf.left(shift),
            KeyCode::Right => buf.right(shift),
            KeyCode::Up => buf.vertical(-1, shift),
            KeyCode::Down => buf.vertical(1, shift),
            KeyCode::PageUp => buf.vertical(-page, shift),
            KeyCode::PageDown => buf.vertical(page, shift),
            KeyCode::Home if ctrl => buf.doc_start(shift),
            KeyCode::End if ctrl => buf.doc_end(shift),
            KeyCode::Home => buf.home(shift),
            KeyCode::End => buf.end(shift),
            _ => {}
        }
        // Editing a preview keeps it open.
        if buf.dirty() {
            of.preview = false;
        }
        if let Some(text) = copied {
            notify::copy(&text);
            self.clip = text;
        }
    }

    fn find_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let (Some(find), Some(of)) = (&mut self.find, self.tabs.get_mut(self.active)) else {
            return;
        };
        let Some(buf) = &mut of.buf else {
            self.find = None;
            return;
        };
        of.follow = true;
        let select = |buf: &mut Buffer, hit: Option<(Pos, Pos)>| match hit {
            Some((a, b)) => {
                buf.move_to(a, false);
                buf.move_to(b, true);
                false
            }
            None => true,
        };
        let step = match k.code {
            KeyCode::Esc => {
                self.find = None;
                return;
            }
            KeyCode::Enter => Some(!shift),
            KeyCode::Down => Some(true),
            KeyCode::Up => Some(false),
            KeyCode::Char('f') if ctrl => Some(true),
            _ => None,
        };
        let q = find.input.text.clone();
        match step {
            Some(forward) => {
                let from = if forward {
                    buf.cur
                } else {
                    buf.selection().map_or(buf.cur, |(a, _)| a)
                };
                let hit = buf.find(&q, from, forward);
                find.miss = select(buf, hit);
            }
            None => {
                find.input.handle_key(&k);
                if find.input.text != q {
                    let hit = buf.find(&find.input.text, find.origin, true);
                    find.miss = select(buf, hit);
                    if hit.is_none() {
                        buf.move_to(find.origin, false);
                    }
                    find.miss &= !find.input.text.is_empty();
                }
            }
        }
    }

    // ---------- mouse ----------

    /// The buffer position under the mouse in the text area, past the edges if it's outside.
    fn pos_at(&self, x: u16, y: u16) -> Option<Pos> {
        let of = self.file()?;
        let buf = of.buf.as_ref()?;
        let r = self.text_rect;
        let row = (of.top as isize + y as isize - r.y as isize)
            .clamp(0, buf.lines.len() as isize - 1) as usize;
        let col = of.left + x.saturating_sub(r.x) as usize;
        Some(Pos::new(row, buffer::col_at(&buf.lines[row], col)))
    }

    pub fn handle_mouse(&mut self, m: MouseEvent, theme: &Theme) -> MouseTarget {
        let (x, y) = (m.column, m.row);
        let inside = |r: Rect| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height;
        // The save-changes dialog takes every click until it's answered.
        if self.pending.is_some() {
            if m.kind == MouseEventKind::Down(MouseButton::Left)
                && let Some(&(_, answer)) = self.buttons.iter().find(|(r, _)| inside(*r))
                && let Some(p) = self.pending.take()
                && let EditorAction::Close = self.answer(p, answer, theme)
            {
                return MouseTarget::Close;
            }
            return MouseTarget::Editor;
        }
        let in_claude = self.claude_shown() && inside(self.claude_rect);
        let tree_sep = self.tree_rect.x + self.tree_rect.width;
        let claude_sep = self.claude_rect.x;
        let on_sep =
            |sep: u16, r: Rect| (x == sep || x + 1 == sep) && y >= r.y && y < r.y + r.height;
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.claude_shown() && on_sep(claude_sep, self.claude_rect) {
                    self.drag = Some(Drag::ClaudeSeparator);
                } else if on_sep(tree_sep, self.tree_rect) {
                    self.drag = Some(Drag::TreeSeparator);
                } else if in_claude {
                    self.focus = Focus::Claude;
                    return MouseTarget::Claude;
                } else if inside(self.tree_rect) {
                    self.focus = Focus::Tree;
                    let r = self.tree_inner;
                    let i = self.tree_offset + y.saturating_sub(r.y) as usize;
                    if inside(r) && i < self.rows.len() {
                        self.sel = i;
                        if self.rows[i].dir {
                            self.toggle_dir(i);
                        } else {
                            let path = self.rows[i].key.clone();
                            self.open_path(path, true, false, theme);
                        }
                    }
                } else if inside(self.tab_row) {
                    if let Some(hit) = self.tab_hits.iter().find(|h| x >= h.x && x < h.end) {
                        let i = hit.tab;
                        if x == hit.close {
                            self.close_tab(i);
                        } else {
                            self.tabs[i].preview = false;
                            self.activate(i);
                            self.focus = Focus::File;
                        }
                    }
                } else if inside(self.file_rect) {
                    self.focus = Focus::File;
                    if inside(self.text_rect)
                        && let Some(p) = self.pos_at(x, y)
                        && let Some(of) = self.tabs.get_mut(self.active)
                        && let Some(buf) = &mut of.buf
                    {
                        buf.move_to(p, false);
                        self.drag = Some(Drag::Select);
                    }
                }
            }
            MouseEventKind::Down(_) if in_claude => return MouseTarget::Claude,
            // A middle click closes a tab, as in most editors.
            MouseEventKind::Down(MouseButton::Middle) if inside(self.tab_row) => {
                if let Some(hit) = self.tab_hits.iter().find(|h| x >= h.x && x < h.end) {
                    self.close_tab(hit.tab);
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => match self.drag {
                Some(Drag::TreeSeparator) => {
                    self.tree_width = x.saturating_sub(self.tree_rect.x).clamp(16, 100);
                }
                Some(Drag::ClaudeSeparator) => {
                    let right = self.claude_rect.x + self.claude_rect.width;
                    self.claude_width = right.saturating_sub(x).clamp(24, 200);
                }
                Some(Drag::Select) => {
                    if let Some(p) = self.pos_at(x, y)
                        && let Some(of) = self.tabs.get_mut(self.active)
                        && let Some(buf) = &mut of.buf
                    {
                        buf.move_to(p, true);
                        of.follow = true;
                    }
                }
                None => {}
            },
            MouseEventKind::Up(_) => {
                if self.drag == Some(Drag::Select) {
                    let text = self
                        .file()
                        .and_then(|f| f.buf.as_ref())
                        .and_then(|b| b.selected_text());
                    if let Some(text) = text {
                        notify::copy(&text);
                        self.clip = text;
                    }
                }
                self.drag = None;
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let up = m.kind == MouseEventKind::ScrollUp;
                if in_claude {
                    return MouseTarget::Claude;
                } else if inside(self.tree_rect) {
                    let last = self.rows.len().saturating_sub(1);
                    self.sel = if up {
                        self.sel.saturating_sub(1)
                    } else {
                        (self.sel + 1).min(last)
                    };
                } else if inside(self.file_rect)
                    && let Some(of) = self.tabs.get_mut(self.active)
                    && let Some(buf) = &of.buf
                {
                    let top = of.top as isize + if up { -3 } else { 3 };
                    of.top = top.clamp(0, buf.lines.len() as isize - 1) as usize;
                    of.follow = false;
                }
            }
            _ => {}
        }
        MouseTarget::Editor
    }

    // ---------- drawing ----------

    /// Draws the tree, the file and the key hints over `area`, and returns where the claude
    /// panel goes, if there's room for it.
    pub fn draw(
        &mut self,
        f: &mut Frame,
        area: Rect,
        keys: &GlobalKeys,
        t: &Theme,
    ) -> Option<Rect> {
        f.render_widget(Clear, area);
        f.render_widget(ui::fill(t.bg), area);
        let body = Rect {
            height: area.height.saturating_sub(1),
            ..area
        };
        let tw = self
            .tree_width
            .min(body.width.saturating_sub(30))
            .max(16)
            .min(body.width);
        let rest = body.width - tw;
        let want = match self.claude_width {
            // As wide as the file tree unless dragged.
            0 => tw,
            w => w,
        };
        let mut cw = want.min(rest.saturating_sub(30));
        if cw < 24 {
            cw = 0;
        }
        self.tree_rect = Rect { width: tw, ..body };
        self.file_rect = Rect {
            x: body.x + tw,
            width: rest - cw,
            ..body
        };
        self.claude_rect = Rect {
            x: body.x + tw + rest - cw,
            width: cw,
            ..body
        };
        if cw == 0 && self.focus == Focus::Claude {
            self.focus = Focus::File;
        }
        self.draw_tree(f, t);
        self.draw_file(f, t);
        self.draw_hints(
            f,
            Rect {
                y: area.y + area.height.saturating_sub(1),
                height: 1,
                ..area
            },
            keys,
            t,
        );
        self.draw_dialog(f, t);
        (cw > 0).then_some(self.claude_rect)
    }

    fn draw_tree(&mut self, f: &mut Frame, t: &Theme) {
        let focused = self.focus == Focus::Tree && self.pending.is_none();
        let block = ui::panel(&format!(" Files · {} ", tilde(&self.root)), focused, t);
        let mut inner = block.inner(self.tree_rect);
        f.render_widget(block, self.tree_rect);
        let muted = Style::default().fg(t.muted);
        if let Some(input) = &self.filter {
            let (lines, (_, ccol)) = input.layout(inner.width.saturating_sub(3) as usize);
            let text = lines.last().cloned().unwrap_or_default();
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(" / ", Style::default().fg(t.accent)),
                    Span::styled(text, Style::default().fg(t.fg)),
                ])),
                Rect { height: 1, ..inner },
            );
            if focused {
                f.set_cursor_position((inner.x + 3 + ccol as u16, inner.y));
            }
            inner.y += 1;
            inner.height = inner.height.saturating_sub(1);
        }
        self.tree_inner = inner;
        let h = inner.height as usize;
        if self.sel < self.tree_offset {
            self.tree_offset = self.sel;
        } else if h > 0 && self.sel >= self.tree_offset + h {
            self.tree_offset = self.sel + 1 - h;
        }
        self.tree_offset = self.tree_offset.min(self.rows.len().saturating_sub(1));
        let empty = if !self.loaded {
            Some(" Loading…")
        } else if self.rows.is_empty() && self.filter.is_some() {
            Some(" No matches")
        } else if self.rows.is_empty() {
            Some(" No files")
        } else {
            None
        };
        if let Some(msg) = empty {
            f.render_widget(Paragraph::new(Line::styled(msg, muted)), inner);
            return;
        }
        let open = self.file().map(|f| f.path.as_str());
        let unsaved: Vec<&str> = self
            .tabs
            .iter()
            .filter(|f| f.dirty())
            .map(|f| f.path.as_str())
            .collect();
        let width = inner.width as usize;
        let mut lines = Vec::new();
        for (i, r) in self.rows.iter().enumerate().skip(self.tree_offset).take(h) {
            let base = if i == self.sel {
                Style::default().bg(t.selection).fg(t.fg)
            } else {
                Style::default().fg(t.fg)
            };
            let mut spans = vec![Span::styled(format!(" {}", "  ".repeat(r.depth)), base)];
            let right = if r.dir {
                spans.push(Span::styled(
                    if r.open { "▾ " } else { "▸ " },
                    base.fg(t.muted),
                ));
                spans.push(Span::styled(r.label.clone(), base));
                // Folders holding unsaved files get a filled dot, those holding git changes a
                // small one.
                if unsaved.iter().any(|p| p.starts_with(&r.key)) {
                    vec![Span::styled(" ● ", base.fg(t.fg))]
                } else if self.changed_dirs.contains(&r.key) {
                    vec![Span::styled(" • ", base.fg(t.yellow))]
                } else {
                    vec![]
                }
            } else {
                let status = self.changes.get(&r.key).copied();
                let color = match status {
                    Some('A') | Some('?') => t.green,
                    Some('D') => t.red,
                    Some('R') => t.blue,
                    Some(_) => t.yellow,
                    None => t.fg,
                };
                let mut style = base.fg(color);
                if open == Some(r.key.as_str()) {
                    style = style.add_modifier(Modifier::BOLD);
                }
                spans.push(Span::styled(r.label.clone(), style));
                let mut right = vec![];
                if unsaved.contains(&r.key.as_str()) {
                    right.push(Span::styled(" ●", base.fg(t.fg)));
                }
                if let Some(s) = status {
                    let letter = if s == '?' { 'A' } else { s };
                    right.push(Span::styled(format!(" {letter}"), base.fg(color)));
                }
                if !right.is_empty() {
                    right.push(Span::styled(" ", base));
                }
                right
            };
            let rw: usize = right.iter().map(|s| s.width()).sum();
            let left = ui::truncate_line(Line::from(spans), width.saturating_sub(rw));
            let pad = width.saturating_sub(left.width() + rw);
            let mut spans = left.spans;
            spans.push(Span::styled(" ".repeat(pad), base));
            spans.extend(right);
            lines.push(Line::from(spans));
        }
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_file(&mut self, f: &mut Frame, t: &Theme) {
        let focused = self.focus == Focus::File && self.pending.is_none();
        let muted = Style::default().fg(t.muted);
        self.tab_hits.clear();
        self.tab_row = Rect::default();
        if self.tabs.is_empty() {
            let block = ui::panel(" Editor ", focused, t);
            let inner = block.inner(self.file_rect);
            f.render_widget(block, self.file_rect);
            self.text_rect = Rect::default();
            let msg = Line::styled("Pick a file on the left", muted).centered();
            f.render_widget(
                Paragraph::new(msg),
                Rect {
                    y: inner.y + inner.height / 2,
                    height: 1,
                    ..inner
                },
            );
            return;
        }
        self.active = self.active.min(self.tabs.len() - 1);
        let of = &self.tabs[self.active];
        let mut title = format!(" {}{} ", of.path, if of.dirty() { " ●" } else { "" });
        if of.stale {
            title += "· changed on disk ";
        }
        let block = ui::panel(&title, focused, t);
        let mut inner = block.inner(self.file_rect);
        f.render_widget(block, self.file_rect);
        if inner.height > 1 {
            self.tab_row = Rect { height: 1, ..inner };
            self.draw_tabs(f, t);
            inner.y += 1;
            inner.height -= 1;
        }
        let of = &mut self.tabs[self.active];
        let Some(buf) = &mut of.buf else {
            self.text_rect = Rect::default();
            let note = of.note.unwrap_or("");
            f.render_widget(Paragraph::new(Line::styled(note, muted)), inner);
            return;
        };
        let find_row = self.find.as_ref().map(|_| {
            inner.height = inner.height.saturating_sub(1);
            Rect {
                y: inner.y + inner.height,
                height: 1,
                ..inner
            }
        });
        let gw = buf.lines.len().to_string().len().max(3) + 2;
        let text = Rect {
            x: inner.x + gw as u16,
            width: inner.width.saturating_sub(gw as u16),
            ..inner
        };
        self.text_rect = text;
        let (h, w) = (text.height as usize, text.width as usize);
        let cur = buf.cur;
        let cur_col = buffer::display_col(&buf.lines[cur.row], cur.col);
        if of.follow {
            if cur.row < of.top {
                of.top = cur.row;
            } else if h > 0 && cur.row >= of.top + h {
                of.top = cur.row + 1 - h;
            }
            if cur_col < of.left {
                of.left = cur_col;
            } else if w > 0 && cur_col >= of.left + w {
                of.left = cur_col + 1 - w;
            }
            of.follow = false;
        }
        of.top = of.top.min(buf.lines.len().saturating_sub(1));
        let hl = of
            .hl
            .get_or_insert_with(|| Highlighter::new(&of.path, t, buf.lines.len()));
        if let Some(row) = buf.take_changed() {
            hl.invalidate(row);
        }
        hl.ensure(&buf.lines, of.top + h);
        let sel = buf.selection();
        let mut lines = Vec::new();
        for row in of.top..(of.top + h).min(buf.lines.len()) {
            let num_style = if row == cur.row {
                Style::default().fg(t.accent)
            } else {
                Style::default().fg(t.border)
            };
            let mut spans = vec![Span::styled(
                format!(" {:>n$} ", row + 1, n = gw - 2),
                num_style,
            )];
            let range = sel.and_then(|(a, b)| {
                (row >= a.row && row <= b.row).then(|| {
                    let start = if row == a.row { a.col } else { 0 };
                    let end = if row == b.row { b.col } else { usize::MAX };
                    (start, end, row < b.row)
                })
            });
            spans.extend(render_line(&hl.lines[row], range, of.left, w, t.selection));
            lines.push(Line::from(spans));
        }
        f.render_widget(Paragraph::new(lines), inner);
        let in_view = cur.row >= of.top && cur.row < of.top + h && cur_col >= of.left;
        if focused && self.find.is_none() && in_view && cur_col - of.left < w {
            f.set_cursor_position((
                text.x + (cur_col - of.left) as u16,
                text.y + (cur.row - of.top) as u16,
            ));
        }
        if let (Some(find), Some(r)) = (&self.find, find_row) {
            let label = " find ";
            let room = (r.width as usize).saturating_sub(label.len() + 14);
            let (lines, (_, ccol)) = find.input.layout(room);
            let mut spans = vec![
                Span::styled(label, Style::default().bg(t.accent).fg(t.bg)),
                Span::styled(" ", Style::default()),
                Span::styled(
                    lines.last().cloned().unwrap_or_default(),
                    Style::default().fg(t.fg),
                ),
            ];
            if find.miss {
                spans.push(Span::styled("  no matches", Style::default().fg(t.red)));
            }
            f.render_widget(Paragraph::new(Line::from(spans)), r);
            if focused {
                f.set_cursor_position((r.x + label.len() as u16 + 1 + ccol as u16, r.y));
            }
        }
    }

    /// The open files as tabs along `tab_row`: the shown one highlighted, a dot before the
    /// close mark on those with unsaved changes, previews in italics. Scrolls to keep the
    /// shown tab in view when they don't all fit.
    fn draw_tabs(&mut self, f: &mut Frame, t: &Theme) {
        let r = self.tab_row;
        let labels = tab_labels(&self.tabs);
        let widths: Vec<usize> = labels
            .iter()
            .zip(&self.tabs)
            .map(|(l, of)| l.chars().count() + if of.dirty() { 6 } else { 4 })
            .collect();
        let width = r.width as usize;
        self.tab_offset = self.tab_offset.min(self.active);
        while self.tab_offset < self.active
            && widths[self.tab_offset..=self.active].iter().sum::<usize>() > width
        {
            self.tab_offset += 1;
        }
        let mut spans = vec![];
        let mut x = r.x;
        for (i, label) in labels.iter().enumerate().skip(self.tab_offset) {
            let of = &self.tabs[i];
            let shown = i == self.active;
            let mut style = if shown {
                Style::default().bg(t.selection).fg(t.fg)
            } else {
                Style::default().fg(t.muted)
            };
            if shown && self.focus == Focus::File {
                style = style.add_modifier(Modifier::BOLD);
            }
            let mut name = style;
            if of.stale {
                name = name.fg(t.yellow);
            }
            if of.preview {
                name = name.add_modifier(Modifier::ITALIC);
            }
            let w = widths[i] as u16;
            spans.push(Span::styled(" ", style));
            spans.push(Span::styled(label.clone(), name));
            spans.push(Span::styled(" ", style));
            if of.dirty() {
                let dot = style.fg(if shown { t.accent } else { t.fg });
                spans.push(Span::styled("●", dot));
                spans.push(Span::styled(" ", style));
            }
            // Closing a tab with unsaved changes asks to save or discard them first.
            spans.push(Span::styled("×", style.fg(t.muted)));
            spans.push(Span::styled(" ", style));
            self.tab_hits.push(TabHit {
                tab: i,
                x,
                end: x + w,
                close: x + w - 2,
            });
            x += w;
            if x >= r.x + r.width {
                break;
            }
        }
        f.render_widget(Paragraph::new(Line::from(spans)), r);
    }

    /// What the save-changes prompt asks.
    fn question(&self, p: &Pending) -> String {
        let name = match p {
            Pending::CloseTab(path) => path.clone(),
            Pending::Close => match self.dirty_count() {
                1 => self
                    .tabs
                    .iter()
                    .find(|f| f.dirty())
                    .map_or("the file".into(), |f| f.path.clone()),
                n => format!("{n} files"),
            },
        };
        format!("Save changes to {name}?")
    }

    /// The save-changes prompt as a dialog over the file, with buttons to click.
    fn draw_dialog(&mut self, f: &mut Frame, t: &Theme) {
        self.buttons.clear();
        let Some(p) = &self.pending else { return };
        let question = self.question(p);
        let cancel = match p {
            Pending::Close => "Keep editing (esc)",
            Pending::CloseTab(_) => "Cancel (esc)",
        };
        let labels = [
            (" Save (y) ".to_string(), Answer::Save),
            (" Don't save (n) ".to_string(), Answer::Discard),
            (format!(" {cancel} "), Answer::Cancel),
        ];
        let row_w: usize = labels.iter().map(|(l, _)| l.chars().count()).sum::<usize>() + 4;
        let area = self.file_rect;
        let w = (question.chars().count().max(row_w) as u16 + 6).min(area.width);
        let h = 7.min(area.height);
        let r = Rect {
            x: area.x + (area.width - w) / 2,
            y: area.y + (area.height - h) / 2,
            width: w,
            height: h,
        };
        f.render_widget(Clear, r);
        let block = ui::modal_block(" Unsaved changes ", t);
        let inner = block.inner(r);
        f.render_widget(block, r);
        if inner.height < 4 {
            return;
        }
        let text = Rect {
            y: inner.y + 1,
            height: 1,
            ..inner
        };
        f.render_widget(Paragraph::new(Line::raw(question).centered()), text);
        let y = inner.y + 3;
        let mut x = inner.x + (inner.width.saturating_sub(row_w as u16 - 2)) / 2;
        for (label, answer) in labels {
            let style = if answer == Answer::Save {
                Style::default()
                    .bg(t.accent)
                    .fg(t.bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().bg(t.selection).fg(t.fg)
            };
            let bw = (label.chars().count() as u16).min((inner.x + inner.width).saturating_sub(x));
            let b = Rect {
                x,
                y,
                width: bw,
                height: 1,
            };
            f.render_widget(Paragraph::new(Line::styled(label, style)), b);
            self.buttons.push((b, answer));
            x += bw + 2;
        }
    }

    fn draw_hints(&self, f: &mut Frame, r: Rect, keys: &GlobalKeys, t: &Theme) {
        let key = Style::default().fg(t.accent).add_modifier(Modifier::BOLD);
        let muted = Style::default().fg(t.muted);
        let mut spans = vec![Span::raw(" ")];
        let add = |spans: &mut Vec<Span>, k: String, label: &str| {
            spans.push(Span::styled(k, key));
            spans.push(Span::styled(format!(" {label}  "), muted));
        };
        let close = keys.editor.short();
        let panels = format!("{}{}", keys.focus_list.short(), keys.focus_pane.short());
        if let Some(p) = &self.pending {
            spans.push(Span::styled(
                format!("{}  ", self.question(p)),
                Style::default().fg(t.yellow),
            ));
            add(&mut spans, "y".into(), "save");
            add(&mut spans, "n".into(), "discard");
            add(
                &mut spans,
                "esc".into(),
                match p {
                    Pending::Close => "keep editing",
                    Pending::CloseTab(_) => "cancel",
                },
            );
        } else {
            match self.focus {
                Focus::Tree if self.filter.is_some() => {
                    add(&mut spans, "↑↓".into(), "move");
                    add(&mut spans, "⏎".into(), "open");
                    add(&mut spans, "esc".into(), "clear filter");
                }
                Focus::Tree => {
                    add(&mut spans, "j/k".into(), "move");
                    add(&mut spans, "⏎".into(), "open");
                    add(&mut spans, "/".into(), "filter");
                    add(&mut spans, keys.editor_claude.short(), "claude");
                    add(&mut spans, "r".into(), "refresh");
                    add(&mut spans, panels, "panels");
                    add(&mut spans, format!("esc {close}"), "close");
                }
                Focus::File if self.find.is_some() => {
                    add(&mut spans, "⏎ ↓".into(), "next");
                    add(&mut spans, "⇧⏎ ↑".into(), "previous");
                    add(&mut spans, "esc".into(), "done");
                }
                Focus::File => {
                    add(&mut spans, "^s".into(), "save");
                    add(&mut spans, "^z ^y".into(), "undo/redo");
                    add(&mut spans, "^f".into(), "find");
                    add(&mut spans, "^w".into(), "close tab");
                    add(&mut spans, "^PgUp ^PgDn".into(), "tabs");
                    add(&mut spans, keys.editor_claude.short(), "claude");
                    add(&mut spans, "^c ^x ^v".into(), "copy/cut/paste");
                    add(&mut spans, panels, "panels");
                    add(&mut spans, "esc".into(), "files");
                    add(&mut spans, close, "close");
                }
                Focus::Claude => {
                    add(&mut spans, keys.editor_claude.short(), "back to the file");
                    add(&mut spans, close, "close");
                    spans.push(Span::styled("everything else goes to claude", muted));
                }
            }
        }
        let right = self.message.as_ref().map(|(msg, err, _)| {
            Span::styled(
                format!("{msg} "),
                Style::default().fg(if *err { t.red } else { t.green }),
            )
        });
        let rw = right.as_ref().map_or(0, |s| s.width());
        let width = r.width as usize;
        let left = ui::truncate_line(Line::from(spans), width.saturating_sub(rw + 1));
        let pad = width.saturating_sub(left.width() + rw);
        let mut all = left.spans;
        all.push(Span::raw(" ".repeat(pad)));
        all.extend(right);
        f.render_widget(Paragraph::new(Line::from(all)), r);
    }
}

fn file_row(path: &str) -> TreeRow {
    TreeRow {
        depth: 0,
        label: path.to_string(),
        key: path.to_string(),
        dir: false,
        open: false,
    }
}

/// Each tab's name: the file name, with enough of its folders to tell apart tabs that share
/// one.
fn tab_labels(tabs: &[OpenFile]) -> Vec<String> {
    let parts: Vec<Vec<&str>> = tabs.iter().map(|f| f.path.rsplit('/').collect()).collect();
    let label = |p: &[&str], n: usize| {
        let mut v: Vec<&str> = p.iter().take(n).copied().collect();
        v.reverse();
        v.join("/")
    };
    (0..tabs.len())
        .map(|i| {
            let mut n = 1;
            while n < parts[i].len()
                && (0..tabs.len()).any(|j| j != i && label(&parts[j], n) == label(&parts[i], n))
            {
                n += 1;
            }
            label(&parts[i], n)
        })
        .collect()
}

fn walk_tree(
    n: &Node,
    prefix: &str,
    depth: usize,
    expanded: &HashSet<String>,
    out: &mut Vec<TreeRow>,
) {
    for (name, child) in &n.dirs {
        // Compact single-directory chains: a/b/c
        let mut label = name.clone();
        let mut node = child;
        while node.files.is_empty() && node.dirs.len() == 1 {
            let (n2, c2) = node.dirs.iter().next().unwrap();
            label = format!("{label}/{n2}");
            node = c2;
        }
        let key = format!("{prefix}{label}/");
        let open = expanded.contains(&key);
        out.push(TreeRow {
            depth,
            label,
            key: key.clone(),
            dir: true,
            open,
        });
        if open {
            walk_tree(node, &key, depth + 1, expanded, out);
        }
    }
    for name in &n.files {
        out.push(TreeRow {
            depth,
            label: name.clone(),
            key: format!("{prefix}{name}"),
            dir: false,
            open: false,
        });
    }
}

/// One buffer line's spans: highlighted, tabs expanded, `width` columns from column `left`.
/// `sel` is a selected byte range on the line, and whether the selection runs past its end.
fn render_line(
    spans: &HlLine,
    sel: Option<(usize, usize, bool)>,
    left: usize,
    width: usize,
    sel_bg: Color,
) -> Vec<Span<'static>> {
    let mut out: Vec<Span<'static>> = vec![];
    let mut push = |style: Style, s: &str| match out.last_mut() {
        Some(last) if last.style == style => last.content.to_mut().push_str(s),
        _ => out.push(Span::styled(s.to_string(), style)),
    };
    let right = left + width;
    let (mut byte, mut col) = (0, 0);
    'outer: for (style, text) in spans {
        for ch in text.chars() {
            if col >= right {
                break 'outer;
            }
            let cw = buffer::char_width(ch, col);
            let selected = sel.is_some_and(|(a, b, _)| byte >= a && byte < b);
            let st = if selected { style.bg(sel_bg) } else { *style };
            if ch != '\t' && col >= left && col + cw <= right {
                let mut buf = [0u8; 4];
                push(st, ch.encode_utf8(&mut buf));
            } else {
                // Tabs, and wide characters cut by an edge, as spaces.
                for c in col..col + cw {
                    if c >= left && c < right {
                        push(st, " ");
                    }
                }
            }
            byte += ch.len_utf8();
            col += cw;
        }
    }
    if sel.is_some_and(|(_, _, eol)| eol) && col >= left && col < right {
        push(Style::default().bg(sel_bg), " ");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(spans: &[Span]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn renders_tabs_and_clips_columns() {
        let line: HlLine = vec![(Style::default(), "\tab".into())];
        assert_eq!(
            text(&render_line(&line, None, 0, 10, Color::Reset)),
            "    ab"
        );
        assert_eq!(text(&render_line(&line, None, 2, 3, Color::Reset)), "  a");
    }

    #[test]
    fn marks_the_selection_and_line_end() {
        let line: HlLine = vec![(Style::default(), "abc".into())];
        let spans = render_line(&line, Some((1, usize::MAX, true)), 0, 10, Color::Red);
        let sel: String = spans
            .iter()
            .filter(|s| s.style.bg == Some(Color::Red))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(sel, "bc ");
    }

    #[test]
    fn tree_compacts_and_expands() {
        let mut e = Editor::new(
            PathBuf::from("/tmp"),
            None,
            crate::theme::by_id(crate::theme::DEFAULT_ID),
            30,
            0,
        );
        e.set_files(
            vec![
                "README.md".into(),
                "src/a/b.rs".into(),
                "src/main.rs".into(),
            ],
            vec![git::Change {
                path: "src/main.rs".into(),
                old_path: None,
                status: 'M',
            }],
        );
        let keys: Vec<&str> = e.rows.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(keys, ["src/", "README.md"]);
        assert!(e.changed_dirs.contains("src/"));
        e.reveal("src/a/b.rs");
        let keys: Vec<&str> = e.rows.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(
            keys,
            ["src/", "src/a/", "src/a/b.rs", "src/main.rs", "README.md"]
        );
        assert_eq!(e.rows[e.sel].key, "src/a/b.rs");
    }

    fn open_file(path: &str) -> OpenFile {
        OpenFile {
            path: path.into(),
            buf: Some(Buffer::new("x")),
            note: None,
            mtime: None,
            stale: false,
            top: 0,
            left: 0,
            follow: false,
            hl: None,
            preview: false,
        }
    }

    #[test]
    fn tab_labels_tell_same_names_apart() {
        let tabs = [
            open_file("src/app.rs"),
            open_file("src/ui/mod.rs"),
            open_file("src/git/mod.rs"),
            open_file("README.md"),
        ];
        assert_eq!(
            tab_labels(&tabs),
            ["app.rs", "ui/mod.rs", "git/mod.rs", "README.md"]
        );
    }

    #[test]
    fn opens_tabs_previews_and_closes() {
        let dir = std::env::temp_dir().join(format!("maximus-tabs-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        for name in ["a.rs", "b.rs", "c.rs"] {
            fs::write(dir.join(name), "fn main() {}\n").unwrap();
        }
        let theme = crate::theme::by_id(crate::theme::DEFAULT_ID);
        let mut e = Editor::new(dir.clone(), None, theme, 30, 0);
        let paths = |e: &Editor| e.tabs.iter().map(|f| f.path.clone()).collect::<Vec<_>>();

        e.open_path("a.rs".into(), true, false, theme);
        // A preview opens beside the shown tab, and the next preview takes its place.
        e.open_path("b.rs".into(), false, true, theme);
        e.open_path("c.rs".into(), false, true, theme);
        assert_eq!(paths(&e), ["a.rs", "c.rs"]);
        assert_eq!(e.active, 1);

        // An edited preview stays put.
        e.tabs[1].buf.as_mut().unwrap().insert("// ");
        e.tabs[1].preview = false;
        e.open_path("b.rs".into(), false, true, theme);
        assert_eq!(paths(&e), ["a.rs", "c.rs", "b.rs"]);
        assert_eq!(e.dirty_count(), 1);

        // Opening an open file shows its tab.
        e.open_path("a.rs".into(), true, false, theme);
        assert_eq!(e.active, 0);

        // Closing an unsaved tab asks; discarding closes it.
        e.close_tab(1);
        assert!(e.has_prompt());
        assert_eq!(e.active, 1);
        e.handle_key(KeyEvent::from(KeyCode::Char('n')), theme);
        assert_eq!(paths(&e), ["a.rs", "b.rs"]);
        assert_eq!(e.active, 1);

        e.close_tab(1);
        assert_eq!(paths(&e), ["a.rs"]);
        assert_eq!(e.active, 0);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn filter_ranks_matches() {
        let mut e = Editor::new(
            PathBuf::from("/tmp"),
            None,
            crate::theme::by_id(crate::theme::DEFAULT_ID),
            30,
            0,
        );
        e.set_files(
            vec![
                "src/app.rs".into(),
                "src/editor.rs".into(),
                "docs/x.md".into(),
            ],
            vec![],
        );
        e.filter = Some(TextInput::with_text("edit", false));
        e.rebuild_rows();
        assert_eq!(e.rows[0].key, "src/editor.rs");
    }
}
