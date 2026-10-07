use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::OnceLock,
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
};
use similar::{DiffTag, TextDiff};
use syntect::{
    easy::HighlightLines, highlighting::Theme as SynTheme, parsing::SyntaxSet,
    util::LinesWithEndings,
};
use two_face::re_exports::syntect;
use unicode_width::UnicodeWidthChar;

use crate::{config::tilde, git, theme::Theme, ui};

const MAX_BYTES: usize = 2_000_000;

fn syntaxes() -> &'static SyntaxSet {
    static S: OnceLock<SyntaxSet> = OnceLock::new();
    S.get_or_init(two_face::syntax::extra_newlines)
}

fn syn_themes() -> &'static two_face::theme::EmbeddedLazyThemeSet {
    static T: OnceLock<two_face::theme::EmbeddedLazyThemeSet> = OnceLock::new();
    T.get_or_init(two_face::theme::extra)
}

/// Warm the syntax set off the UI thread so the first diff opens instantly.
pub fn preload() {
    std::thread::spawn(|| {
        syntaxes();
    });
}

struct FileEntry {
    change: git::Change,
    adds: usize,
    dels: usize,
    old: String,
    new: String,
    note: Option<&'static str>,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Equal,
    Change,
}

enum Row {
    Hunk(String),
    Pair {
        left: Option<usize>,
        right: Option<usize>,
        kind: Kind,
    },
}

type HlLine = Vec<(Style, String)>;

struct FileDiff {
    split: Vec<Row>,
    unified: Vec<Row>,
    old_hl: Vec<HlLine>,
    new_hl: Vec<HlLine>,
}

struct TreeRow {
    depth: usize,
    label: String,
    key: String,
    file: Option<usize>,
}

pub enum DiffAction {
    None,
    Close,
}

pub struct DiffView {
    pub root: PathBuf,
    branch: Option<String>,
    files: Vec<FileEntry>,
    rows: Vec<TreeRow>,
    collapsed: HashSet<String>,
    sel: usize,
    tree_offset: usize,
    focus_file: bool,
    pub split: bool,
    scroll: usize,
    hscroll: usize,
    cache: HashMap<usize, FileDiff>,
    pub tree_width: u16,
    tree_rect: Rect,
    file_rect: Rect,
    dragging: bool,
    error: Option<String>,
    syntax_theme: two_face::theme::EmbeddedThemeName,
}

impl DiffView {
    pub fn open(dir: PathBuf, theme: &Theme, tree_width: u16, split: bool) -> Self {
        let root = git::toplevel(&dir).unwrap_or(dir);
        let mut v = Self {
            branch: git::current_branch(&root),
            root,
            files: vec![],
            rows: vec![],
            collapsed: HashSet::new(),
            sel: 0,
            tree_offset: 0,
            focus_file: false,
            split,
            scroll: 0,
            hscroll: 0,
            cache: HashMap::new(),
            tree_width,
            tree_rect: Rect::default(),
            file_rect: Rect::default(),
            dragging: false,
            error: None,
            syntax_theme: theme.syntax,
        };
        v.reload();
        v
    }

    pub fn reload(&mut self) {
        let keep = self
            .selected_file()
            .map(|f| self.files[f].change.path.clone());
        self.files.clear();
        self.cache.clear();
        self.error = None;
        match git::changes(&self.root) {
            Ok(changes) => {
                for c in changes {
                    self.files.push(load_entry(&self.root, c));
                }
            }
            Err(e) => self.error = Some(format!("{e}")),
        }
        self.rebuild_rows();
        self.sel = keep
            .and_then(|p| {
                self.rows
                    .iter()
                    .position(|r| r.file.is_some_and(|f| self.files[f].change.path == p))
            })
            .or_else(|| self.rows.iter().position(|r| r.file.is_some()))
            .unwrap_or(0);
        self.scroll = 0;
    }

    fn rebuild_rows(&mut self) {
        #[derive(Default)]
        struct Node {
            dirs: BTreeMap<String, Node>,
            files: Vec<(String, usize)>,
        }
        let mut root = Node::default();
        for (i, f) in self.files.iter().enumerate() {
            let parts: Vec<&str> = f.change.path.split('/').collect();
            let mut n = &mut root;
            for d in &parts[..parts.len() - 1] {
                n = n.dirs.entry(d.to_string()).or_default();
            }
            n.files.push((parts[parts.len() - 1].to_string(), i));
        }
        fn walk(
            n: &Node,
            prefix: &str,
            depth: usize,
            collapsed: &HashSet<String>,
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
                let open = !collapsed.contains(&key);
                out.push(TreeRow {
                    depth,
                    label: format!("{} {label}", if open { "▾" } else { "▸" }),
                    key: key.clone(),
                    file: None,
                });
                if open {
                    walk(node, &key, depth + 1, collapsed, out);
                }
            }
            for (name, i) in &n.files {
                out.push(TreeRow {
                    depth,
                    label: name.clone(),
                    key: format!("{prefix}{name}"),
                    file: Some(*i),
                });
            }
        }
        self.rows.clear();
        walk(&root, "", 0, &self.collapsed, &mut self.rows);
    }

    fn selected_file(&self) -> Option<usize> {
        self.rows.get(self.sel).and_then(|r| r.file)
    }

    fn ensure_diff(&mut self, i: usize) {
        if self.cache.contains_key(&i) {
            return;
        }
        let f = &self.files[i];
        let theme = syn_themes().get(self.syntax_theme).clone();
        let fd = build_diff(&f.old, &f.new, &f.change.path, &theme);
        self.cache.insert(i, fd);
    }

    fn select(&mut self, idx: usize) {
        if idx < self.rows.len() && idx != self.sel {
            self.sel = idx;
            self.scroll = 0;
            self.hscroll = 0;
        }
    }

    fn step_file(&mut self, forward: bool) {
        let n = self.rows.len();
        let mut i = self.sel;
        for _ in 0..n {
            i = if forward {
                (i + 1) % n
            } else {
                (i + n - 1) % n
            };
            if self.rows[i].file.is_some() {
                self.select(i);
                return;
            }
        }
    }

    fn rows_len(&self) -> usize {
        self.selected_file()
            .and_then(|f| self.cache.get(&f))
            .map(|d| {
                if self.split {
                    d.split.len()
                } else {
                    d.unified.len()
                }
            })
            .unwrap_or(0)
    }

    fn jump_hunk(&mut self, forward: bool) {
        let Some(f) = self.selected_file() else {
            return;
        };
        let Some(d) = self.cache.get(&f) else { return };
        let rows = if self.split { &d.split } else { &d.unified };
        let hunks: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| matches!(r, Row::Hunk(_)))
            .map(|(i, _)| i)
            .collect();
        let target = if forward {
            hunks.iter().find(|&&h| h > self.scroll).copied()
        } else {
            hunks.iter().rev().find(|&&h| h < self.scroll).copied()
        };
        if let Some(t) = target {
            self.scroll = t;
        }
    }

    fn toggle_dir(&mut self, idx: usize) {
        let key = self.rows[idx].key.clone();
        if !self.collapsed.remove(&key) {
            self.collapsed.insert(key.clone());
        }
        self.rebuild_rows();
        self.sel = self.rows.iter().position(|r| r.key == key).unwrap_or(0);
    }

    pub fn handle_key(&mut self, k: KeyEvent) -> DiffAction {
        let page = self.file_rect.height.saturating_sub(2).max(1) as usize;
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        // Keys shared by both panes.
        match k.code {
            KeyCode::Char('s') => {
                self.split = !self.split;
                self.scroll = 0;
                return DiffAction::None;
            }
            KeyCode::Char('r') if !ctrl => {
                self.reload();
                return DiffAction::None;
            }
            KeyCode::Char('q') => return DiffAction::Close,
            KeyCode::Char('J') | KeyCode::Char('n') => {
                self.jump_hunk(true);
                return DiffAction::None;
            }
            KeyCode::Char('K') | KeyCode::Char('N') => {
                self.jump_hunk(false);
                return DiffAction::None;
            }
            KeyCode::Char(']') => {
                self.step_file(true);
                return DiffAction::None;
            }
            KeyCode::Char('[') => {
                self.step_file(false);
                return DiffAction::None;
            }
            KeyCode::Char('d') if ctrl => {
                self.scroll = (self.scroll + page / 2).min(self.rows_len().saturating_sub(1));
                return DiffAction::None;
            }
            KeyCode::Char('u') if ctrl => {
                self.scroll = self.scroll.saturating_sub(page / 2);
                return DiffAction::None;
            }
            KeyCode::PageDown => {
                self.scroll = (self.scroll + page).min(self.rows_len().saturating_sub(1));
                return DiffAction::None;
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(page);
                return DiffAction::None;
            }
            _ => {}
        }
        if self.focus_file {
            match k.code {
                KeyCode::Esc | KeyCode::Tab | KeyCode::Char('h') => self.focus_file = false,
                KeyCode::Down | KeyCode::Char('j') => {
                    self.scroll = (self.scroll + 1).min(self.rows_len().saturating_sub(1))
                }
                KeyCode::Up | KeyCode::Char('k') => self.scroll = self.scroll.saturating_sub(1),
                KeyCode::Char(' ') => {
                    self.scroll = (self.scroll + page).min(self.rows_len().saturating_sub(1))
                }
                KeyCode::Left => self.hscroll = self.hscroll.saturating_sub(8),
                KeyCode::Right => self.hscroll += 8,
                KeyCode::Char('g') | KeyCode::Home => self.scroll = 0,
                KeyCode::Char('G') | KeyCode::End => {
                    self.scroll = self.rows_len().saturating_sub(1)
                }
                _ => {}
            }
        } else {
            match k.code {
                KeyCode::Esc => return DiffAction::Close,
                KeyCode::Down | KeyCode::Char('j') => {
                    self.select((self.sel + 1).min(self.rows.len().saturating_sub(1)))
                }
                KeyCode::Up | KeyCode::Char('k') => self.select(self.sel.saturating_sub(1)),
                KeyCode::Char('g') | KeyCode::Home => self.select(0),
                KeyCode::Char('G') | KeyCode::End => self.select(self.rows.len().saturating_sub(1)),
                KeyCode::Enter
                | KeyCode::Char('l')
                | KeyCode::Right
                | KeyCode::Tab
                | KeyCode::Char(' ') => {
                    if let Some(r) = self.rows.get(self.sel) {
                        if r.file.is_some() {
                            if k.code != KeyCode::Char(' ') {
                                self.focus_file = true;
                            }
                        } else {
                            self.toggle_dir(self.sel);
                        }
                    }
                }
                KeyCode::Char('h') | KeyCode::Left => {
                    if let Some(r) = self.rows.get(self.sel)
                        && r.file.is_none()
                        && !self.collapsed.contains(&r.key)
                    {
                        self.toggle_dir(self.sel);
                    }
                }
                _ => {}
            }
        }
        DiffAction::None
    }

    pub fn handle_mouse(&mut self, m: MouseEvent) {
        let (x, y) = (m.column, m.row);
        let inside = |r: Rect| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height;
        let sep = self.tree_rect.x + self.tree_rect.width;
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if (x == sep || x + 1 == sep)
                    && y >= self.tree_rect.y
                    && y < self.tree_rect.y + self.tree_rect.height
                {
                    self.dragging = true;
                } else if inside(self.tree_rect) {
                    self.focus_file = false;
                    let row = y.saturating_sub(self.tree_rect.y + 1) as usize + self.tree_offset;
                    if y > self.tree_rect.y && row < self.rows.len() {
                        if self.rows[row].file.is_none() {
                            self.toggle_dir(row);
                        } else {
                            self.select(row);
                        }
                    }
                } else if inside(self.file_rect) {
                    self.focus_file = true;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) if self.dragging => {
                let left = self.tree_rect.x;
                self.tree_width = x.saturating_sub(left).clamp(16, 100);
            }
            MouseEventKind::Up(_) => self.dragging = false,
            MouseEventKind::ScrollDown => {
                if inside(self.tree_rect) {
                    self.select((self.sel + 1).min(self.rows.len().saturating_sub(1)));
                } else {
                    self.scroll = (self.scroll + 3).min(self.rows_len().saturating_sub(1));
                }
            }
            MouseEventKind::ScrollUp => {
                if inside(self.tree_rect) {
                    self.select(self.sel.saturating_sub(1));
                } else {
                    self.scroll = self.scroll.saturating_sub(3);
                }
            }
            MouseEventKind::ScrollRight => self.hscroll += 4,
            MouseEventKind::ScrollLeft => self.hscroll = self.hscroll.saturating_sub(4),
            _ => {}
        }
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let outer = ui::centered(area, 96, 94);
        f.render_widget(Clear, outer);
        f.render_widget(ui::fill(t.bg), outer);
        let body = Rect {
            height: outer.height.saturating_sub(1),
            ..outer
        };
        let tw = self.tree_width.min(body.width.saturating_sub(30)).max(16);
        self.tree_rect = Rect { width: tw, ..body };
        self.file_rect = Rect {
            x: body.x + tw,
            width: body.width.saturating_sub(tw),
            ..body
        };

        // Tree
        let title = format!(
            " Changes ({}) · {}{} ",
            self.files.len(),
            tilde(&self.root),
            self.branch
                .as_deref()
                .map(|b| format!(" · {b}"))
                .unwrap_or_default()
        );
        let block = ui::panel(&title, !self.focus_file, t);
        let inner = block.inner(self.tree_rect);
        f.render_widget(block, self.tree_rect);
        let h = inner.height as usize;
        if self.sel < self.tree_offset {
            self.tree_offset = self.sel;
        } else if h > 0 && self.sel >= self.tree_offset + h {
            self.tree_offset = self.sel + 1 - h;
        }
        let mut lines = Vec::new();
        if let Some(e) = &self.error {
            lines.push(Line::styled(e.clone(), Style::default().fg(t.red)));
        } else if self.files.is_empty() {
            lines.push(Line::styled(
                "Working tree clean",
                Style::default().fg(t.muted),
            ));
        }
        for (i, r) in self.rows.iter().enumerate().skip(self.tree_offset).take(h) {
            let selected = i == self.sel;
            let base = if selected {
                Style::default().bg(t.selection).fg(t.fg)
            } else {
                Style::default().fg(t.fg)
            };
            let mut spans = vec![Span::styled("  ".repeat(r.depth), base)];
            if let Some(fi) = r.file {
                let fe = &self.files[fi];
                let (letter, color) = match fe.change.status {
                    'A' | '?' => ('A', t.green),
                    'D' => ('D', t.red),
                    'R' => ('R', t.blue),
                    _ => ('M', t.yellow),
                };
                spans.push(Span::styled(format!("{letter} "), base.fg(color)));
                spans.push(Span::styled(r.label.clone(), base));
                let stats = format!(" +{} -{}", fe.adds, fe.dels);
                let used: usize = spans.iter().map(|s| s.width()).sum();
                let pad = (inner.width as usize).saturating_sub(used + stats.len());
                spans.push(Span::styled(" ".repeat(pad), base));
                spans.push(Span::styled(format!(" +{}", fe.adds), base.fg(t.green)));
                spans.push(Span::styled(format!(" -{}", fe.dels), base.fg(t.red)));
            } else {
                spans.push(Span::styled(r.label.clone(), base.fg(t.muted)));
                let used: usize = spans.iter().map(|s| s.width()).sum();
                spans.push(Span::styled(
                    " ".repeat((inner.width as usize).saturating_sub(used)),
                    base,
                ));
            }
            lines.push(Line::from(spans));
        }
        f.render_widget(Paragraph::new(lines), inner);

        // File
        let sel_file = self.selected_file();
        let title = match sel_file {
            Some(i) => {
                let fe = &self.files[i];
                match &fe.change.old_path {
                    Some(old) => format!(" {old} → {} ", fe.change.path),
                    None => format!(" {} ", fe.change.path),
                }
            }
            None => " ".into(),
        };
        let block = ui::panel(&title, self.focus_file, t);
        let inner = block.inner(self.file_rect);
        f.render_widget(block, self.file_rect);
        if let Some(i) = sel_file {
            if let Some(note) = self.files[i].note {
                f.render_widget(
                    Paragraph::new(Line::styled(note, Style::default().fg(t.muted))),
                    inner,
                );
            } else {
                self.ensure_diff(i);
                let max_line = self.files[i]
                    .old
                    .lines()
                    .count()
                    .max(self.files[i].new.lines().count());
                let d = &self.cache[&i];
                let rows = if self.split { &d.split } else { &d.unified };
                self.scroll = self.scroll.min(rows.len().saturating_sub(1));
                draw_rows(
                    f,
                    inner,
                    rows,
                    d,
                    self.scroll,
                    self.hscroll,
                    self.split,
                    max_line,
                    t,
                );
            }
        }

        // Hints
        let hint_area = Rect {
            y: outer.y + outer.height.saturating_sub(1),
            height: 1,
            ..outer
        };
        let hints = if self.focus_file {
            "j/k scroll · J/K hunk · ]/[ file · ←/→ pan · s split/unified · esc tree · q close"
        } else {
            "j/k file · J/K hunk · enter open · h/l fold · s split/unified · r refresh · esc close"
        };
        f.render_widget(
            Paragraph::new(Line::styled(
                format!(" {hints}"),
                Style::default().fg(t.muted),
            )),
            hint_area,
        );
    }
}

fn load_entry(root: &Path, change: git::Change) -> FileEntry {
    let old_bytes = match change.status {
        '?' | 'A' => None,
        _ => git::head_contents(root, change.old_path.as_deref().unwrap_or(&change.path)),
    };
    let new_bytes = match change.status {
        'D' => None,
        _ => std::fs::read(root.join(&change.path)).ok(),
    };
    let mut note = None;
    let is_binary = |b: &Option<Vec<u8>>| {
        b.as_ref()
            .is_some_and(|b| b[..b.len().min(8000)].contains(&0))
    };
    let too_big = |b: &Option<Vec<u8>>| b.as_ref().is_some_and(|b| b.len() > MAX_BYTES);
    if is_binary(&old_bytes) || is_binary(&new_bytes) {
        note = Some("Binary file");
    } else if too_big(&old_bytes) || too_big(&new_bytes) {
        note = Some("File too large to diff");
    } else if change.status != 'D' && new_bytes.is_none() && root.join(&change.path).is_dir() {
        note = Some("Directory (submodule?)");
    }
    let (old, new) = if note.is_some() {
        (String::new(), String::new())
    } else {
        (
            old_bytes
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default(),
            new_bytes
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default(),
        )
    };
    let (mut adds, mut dels) = (0, 0);
    if note.is_none() {
        let d = TextDiff::from_lines(&old, &new);
        for op in d.ops() {
            let (tag, o, n) = op.as_tag_tuple();
            match tag {
                DiffTag::Delete => dels += o.len(),
                DiffTag::Insert => adds += n.len(),
                DiffTag::Replace => {
                    dels += o.len();
                    adds += n.len();
                }
                DiffTag::Equal => {}
            }
        }
    }
    FileEntry {
        change,
        adds,
        dels,
        old,
        new,
        note,
    }
}

fn highlight(text: &str, path: &str, theme: &SynTheme) -> Vec<HlLine> {
    let ss = syntaxes();
    let ext = path.rsplit('.').next().unwrap_or("");
    let name = path.rsplit('/').next().unwrap_or(path);
    let syntax = ss
        .find_syntax_by_extension(name)
        .or_else(|| ss.find_syntax_by_extension(ext))
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    let plain = text.lines().count() > 30_000;
    let mut h = HighlightLines::new(syntax, theme);
    let mut out = Vec::new();
    for line in LinesWithEndings::from(text) {
        let clean = |s: &str| s.trim_end_matches(['\n', '\r']).replace('\t', "    ");
        if plain {
            out.push(vec![(Style::default(), clean(line))]);
            continue;
        }
        match h.highlight_line(line, ss) {
            Ok(ranges) => out.push(
                ranges
                    .into_iter()
                    .map(|(st, s)| {
                        let c = st.foreground;
                        let mut style = Style::default().fg(Color::Rgb(c.r, c.g, c.b));
                        if st
                            .font_style
                            .contains(syntect::highlighting::FontStyle::BOLD)
                        {
                            style = style.add_modifier(Modifier::BOLD);
                        }
                        if st
                            .font_style
                            .contains(syntect::highlighting::FontStyle::ITALIC)
                        {
                            style = style.add_modifier(Modifier::ITALIC);
                        }
                        (style, clean(s))
                    })
                    .collect(),
            ),
            Err(_) => out.push(vec![(Style::default(), clean(line))]),
        }
    }
    out
}

fn build_diff(old: &str, new: &str, path: &str, theme: &SynTheme) -> FileDiff {
    let diff = TextDiff::from_lines(old, new);
    let mut split = Vec::new();
    let mut unified = Vec::new();
    for group in diff.grouped_ops(3) {
        let (Some(first), Some(last)) = (group.first(), group.last()) else {
            continue;
        };
        let (os, oe) = (first.old_range().start, last.old_range().end);
        let (ns, ne) = (first.new_range().start, last.new_range().end);
        let header = format!("@@ -{},{} +{},{} @@", os + 1, oe - os, ns + 1, ne - ns);
        split.push(Row::Hunk(header.clone()));
        unified.push(Row::Hunk(header));
        for op in &group {
            let (tag, o, n) = op.as_tag_tuple();
            match tag {
                DiffTag::Equal => {
                    for (a, b) in o.clone().zip(n.clone()) {
                        split.push(Row::Pair {
                            left: Some(a),
                            right: Some(b),
                            kind: Kind::Equal,
                        });
                        unified.push(Row::Pair {
                            left: Some(a),
                            right: Some(b),
                            kind: Kind::Equal,
                        });
                    }
                }
                _ => {
                    let (ol, nl) = (o.len(), n.len());
                    for i in 0..ol.max(nl) {
                        split.push(Row::Pair {
                            left: (i < ol).then(|| o.start + i),
                            right: (i < nl).then(|| n.start + i),
                            kind: Kind::Change,
                        });
                    }
                    for a in o.clone() {
                        unified.push(Row::Pair {
                            left: Some(a),
                            right: None,
                            kind: Kind::Change,
                        });
                    }
                    for b in n.clone() {
                        unified.push(Row::Pair {
                            left: None,
                            right: Some(b),
                            kind: Kind::Change,
                        });
                    }
                }
            }
        }
    }
    FileDiff {
        split,
        unified,
        old_hl: highlight(old, path, theme),
        new_hl: highlight(new, path, theme),
    }
}

/// Render highlighted spans into `width` cells, skipping `skip` columns.
fn clip(
    spans: &[(Style, String)],
    skip: usize,
    width: usize,
    bg: Option<Color>,
) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut col = 0usize;
    let mut used = 0usize;
    for (style, text) in spans {
        let mut s = String::new();
        for ch in text.chars() {
            let w = ch.width().unwrap_or(0);
            if col < skip {
                col += w;
                continue;
            }
            if used + w > width {
                break;
            }
            s.push(ch);
            used += w;
            col += w;
        }
        if !s.is_empty() {
            let mut st = *style;
            if let Some(bg) = bg {
                st = st.bg(bg);
            }
            out.push(Span::styled(s, st));
        }
    }
    if used < width {
        let mut st = Style::default();
        if let Some(bg) = bg {
            st = st.bg(bg);
        }
        out.push(Span::styled(" ".repeat(width - used), st));
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn draw_rows(
    f: &mut Frame,
    area: Rect,
    rows: &[Row],
    d: &FileDiff,
    scroll: usize,
    hscroll: usize,
    split: bool,
    max_line: usize,
    t: &Theme,
) {
    let gw = max_line.max(1).to_string().len().max(3);
    let w = area.width as usize;
    let muted = Style::default().fg(t.muted);
    let mut lines = Vec::new();
    if rows.is_empty() {
        lines.push(Line::styled("No textual changes", muted));
    }
    for row in rows.iter().skip(scroll).take(area.height as usize) {
        match row {
            Row::Hunk(h) => {
                let text = format!(" {h}");
                let pad = w.saturating_sub(text.chars().count());
                lines.push(Line::from(vec![
                    Span::styled(text, Style::default().fg(t.blue).bg(t.selection)),
                    Span::styled(" ".repeat(pad), Style::default().bg(t.selection)),
                ]));
            }
            Row::Pair { left, right, kind } => {
                let mut spans = Vec::new();
                let side = |spans: &mut Vec<Span<'static>>,
                            idx: Option<usize>,
                            hl: &[HlLine],
                            is_new: bool,
                            width: usize| {
                    let changed = *kind == Kind::Change;
                    let bg = match (idx, changed) {
                        (Some(_), true) => Some(if is_new { t.add_bg } else { t.del_bg }),
                        (None, true) => Some(t.surface),
                        _ => None,
                    };
                    let bgs = |s: Style| if let Some(b) = bg { s.bg(b) } else { s };
                    let num = idx
                        .map(|i| format!("{:>gw$} ", i + 1))
                        .unwrap_or_else(|| " ".repeat(gw + 1));
                    spans.push(Span::styled(num, bgs(muted)));
                    let marker = match (idx, changed) {
                        (Some(_), true) if is_new => {
                            Span::styled("+", bgs(Style::default().fg(t.green)))
                        }
                        (Some(_), true) => Span::styled("-", bgs(Style::default().fg(t.red))),
                        _ => Span::styled(" ", bgs(Style::default())),
                    };
                    spans.push(marker);
                    let content_w = width.saturating_sub(gw + 2);
                    let empty = Vec::new();
                    let line = idx.and_then(|i| hl.get(i)).unwrap_or(&empty);
                    spans.extend(clip(line, hscroll, content_w, bg));
                };
                if split {
                    let half = w.saturating_sub(1) / 2;
                    side(&mut spans, *left, &d.old_hl, false, half);
                    spans.push(Span::styled("│", Style::default().fg(t.border)));
                    side(
                        &mut spans,
                        *right,
                        &d.new_hl,
                        true,
                        w.saturating_sub(1) - half,
                    );
                } else {
                    // Unified: old number, new number, content from whichever side exists.
                    let changed = *kind == Kind::Change;
                    let (is_new, idx, hl) = match (left, right) {
                        (_, Some(r)) => (true, *r, &d.new_hl),
                        (Some(l), None) => (false, *l, &d.old_hl),
                        _ => continue,
                    };
                    let bg = changed.then_some(if is_new { t.add_bg } else { t.del_bg });
                    let bgs = |s: Style| if let Some(b) = bg { s.bg(b) } else { s };
                    let ln = |o: Option<usize>| {
                        o.map(|i| format!("{:>gw$} ", i + 1))
                            .unwrap_or_else(|| " ".repeat(gw + 1))
                    };
                    spans.push(Span::styled(ln(*left), bgs(muted)));
                    spans.push(Span::styled(ln(*right), bgs(muted)));
                    spans.push(match (changed, is_new) {
                        (true, true) => Span::styled("+", bgs(Style::default().fg(t.green))),
                        (true, false) => Span::styled("-", bgs(Style::default().fg(t.red))),
                        _ => Span::raw(" "),
                    });
                    let empty = Vec::new();
                    spans.extend(clip(
                        hl.get(idx).unwrap_or(&empty),
                        hscroll,
                        w.saturating_sub(2 * gw + 3),
                        bg,
                    ));
                }
                lines.push(Line::from(spans));
            }
        }
    }
    f.render_widget(Paragraph::new(lines), area);
}
