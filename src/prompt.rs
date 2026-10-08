use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
};

use crate::{
    config::{ProjectEntry, tilde},
    git,
    textinput::TextInput,
    theme::Theme,
    ui,
};

pub const EFFORTS: &[&str] = &["", "low", "medium", "high", "xhigh", "max"];

#[derive(Debug, Clone, PartialEq)]
pub enum Tree {
    Main,
    Existing { path: PathBuf, branch: String },
    New,
}

#[derive(Debug, Clone)]
enum PickValue {
    Project(PathBuf),
    Tree(Tree),
    Model(String),
}

struct Picker {
    title: &'static str,
    options: Vec<(String, String, PickValue)>,
    filter: TextInput,
    shown: Vec<usize>,
    sel: usize,
}

impl Picker {
    fn new(title: &'static str, options: Vec<(String, String, PickValue)>, sel: usize) -> Self {
        let shown = (0..options.len()).collect();
        Self {
            title,
            options,
            filter: TextInput::new(false),
            shown,
            sel,
        }
    }
    fn refilter(&mut self) {
        let m = SkimMatcherV2::default().ignore_case();
        let q = self.filter.text.trim();
        self.shown = (0..self.options.len())
            .filter(|&i| {
                q.is_empty()
                    || m.fuzzy_match(&format!("{} {}", self.options[i].0, self.options[i].1), q)
                        .is_some()
            })
            .collect();
        self.sel = 0;
    }
}

pub struct Launch {
    pub project: PathBuf,
    pub tree: Tree,
    pub branch: String,
    pub model: String,
    pub effort: String,
    pub prompt: String,
}

pub enum PromptAction {
    None,
    Close,
    Launch(Launch),
}

pub struct Prompt {
    pub project: PathBuf,
    pub tree: Tree,
    branch: TextInput,
    branch_edited: bool,
    editing_branch: bool,
    pub model: String,
    pub effort: String,
    text: TextInput,
    picker: Option<Picker>,
    projects: Vec<ProjectEntry>,
    models: Vec<String>,
    is_repo: bool,
}

impl Prompt {
    pub fn new(
        project: PathBuf,
        projects: Vec<ProjectEntry>,
        models: Vec<String>,
        model: String,
        effort: String,
    ) -> Self {
        let is_repo = git::is_repo(&project);
        Self {
            project,
            tree: Tree::Main,
            branch: TextInput::new(false),
            branch_edited: false,
            editing_branch: false,
            model,
            effort,
            text: TextInput::new(true),
            picker: None,
            projects,
            models,
            is_repo,
        }
    }

    pub fn with_new_worktree(mut self) -> Self {
        if self.is_repo {
            self.tree = Tree::New;
        }
        self
    }

    fn auto_branch(&self) -> String {
        let s = git::slug(&self.text.text, 5);
        if s.is_empty() {
            "mx/session".into()
        } else {
            format!("mx/{s}")
        }
    }

    fn branch_name(&self) -> String {
        if self.branch_edited {
            self.branch.text.trim().to_string()
        } else {
            self.auto_branch()
        }
    }

    pub fn paste(&mut self, s: &str) {
        if let Some(p) = &mut self.picker {
            p.filter.insert_str(s);
            p.refilter();
        } else if self.editing_branch {
            self.branch.insert_str(s);
            self.branch_edited = true;
        } else {
            self.text.insert_str(s);
        }
    }

    fn open_picker(&mut self, which: char) {
        let p = match which {
            'p' => {
                let opts: Vec<_> = self
                    .projects
                    .iter()
                    .map(|p| {
                        (
                            p.name.clone(),
                            tilde(&p.path),
                            PickValue::Project(p.path.clone()),
                        )
                    })
                    .collect();
                let sel = self
                    .projects
                    .iter()
                    .position(|p| p.path == self.project)
                    .unwrap_or(0);
                Picker::new("Project", opts, sel)
            }
            't' => {
                let mut opts = vec![(
                    "main checkout".to_string(),
                    tilde(&self.project),
                    PickValue::Tree(Tree::Main),
                )];
                if self.is_repo {
                    for wt in git::worktrees(&self.project)
                        .into_iter()
                        .filter(|w| !w.main)
                    {
                        opts.push((
                            wt.branch.clone(),
                            tilde(&wt.path),
                            PickValue::Tree(Tree::Existing {
                                path: wt.path,
                                branch: wt.branch,
                            }),
                        ));
                    }
                    opts.push((
                        "+ new worktree".into(),
                        "branch named from your prompt".into(),
                        PickValue::Tree(Tree::New),
                    ));
                }
                let sel = opts
                    .iter()
                    .position(|o| matches!(&o.2, PickValue::Tree(t) if *t == self.tree))
                    .unwrap_or(0);
                Picker::new("Worktree", opts, sel)
            }
            _ => {
                let mut opts: Vec<_> = self
                    .models
                    .iter()
                    .map(|m| (m.clone(), String::new(), PickValue::Model(m.clone())))
                    .collect();
                if !self.models.contains(&self.model) && !self.model.is_empty() {
                    opts.push((
                        self.model.clone(),
                        String::new(),
                        PickValue::Model(self.model.clone()),
                    ));
                }
                let sel = opts.iter().position(|o| o.0 == self.model).unwrap_or(0);
                Picker::new("Model", opts, sel)
            }
        };
        self.picker = Some(p);
    }

    fn cycle_effort(&mut self, d: i32) {
        let i = EFFORTS.iter().position(|e| *e == self.effort).unwrap_or(0) as i32;
        let n = EFFORTS.len() as i32;
        self.effort = EFFORTS[((i + d).rem_euclid(n)) as usize].to_string();
    }

    pub fn handle_key(&mut self, k: KeyEvent) -> PromptAction {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if let Some(p) = &mut self.picker {
            let is_model = p.title == "Model";
            match k.code {
                KeyCode::Esc => self.picker = None,
                KeyCode::Enter => {
                    if let Some(&i) = p.shown.get(p.sel) {
                        match p.options[i].2.clone() {
                            PickValue::Project(path) => {
                                if path != self.project {
                                    self.project = path;
                                    self.is_repo = git::is_repo(&self.project);
                                    self.tree = Tree::Main;
                                }
                            }
                            PickValue::Tree(t) => {
                                self.editing_branch = false;
                                self.tree = t;
                            }
                            PickValue::Model(m) => self.model = m,
                        }
                    }
                    self.picker = None;
                }
                KeyCode::Down => p.sel = (p.sel + 1).min(p.shown.len().saturating_sub(1)),
                KeyCode::Char('n') if ctrl => {
                    p.sel = (p.sel + 1).min(p.shown.len().saturating_sub(1))
                }
                KeyCode::Up => p.sel = p.sel.saturating_sub(1),
                KeyCode::Char('p') if ctrl => p.sel = p.sel.saturating_sub(1),
                KeyCode::Left if is_model => self.cycle_effort(-1),
                KeyCode::Right if is_model => self.cycle_effort(1),
                _ => {
                    let before = p.filter.text.clone();
                    p.filter.handle_key(&k);
                    if p.filter.text != before {
                        p.refilter();
                    }
                }
            }
            return PromptAction::None;
        }
        match k.code {
            KeyCode::Esc if self.editing_branch => self.editing_branch = false,
            KeyCode::Esc => return PromptAction::Close,
            KeyCode::Char('p') if ctrl => self.open_picker('p'),
            KeyCode::Char('t') if ctrl => self.open_picker('t'),
            KeyCode::Char('o') if ctrl => self.open_picker('o'),
            KeyCode::Tab if self.tree == Tree::New => {
                if !self.editing_branch && !self.branch_edited {
                    self.branch.set(&self.auto_branch());
                }
                self.editing_branch = !self.editing_branch;
            }
            KeyCode::Enter
                if !k.modifiers.intersects(
                    KeyModifiers::ALT | KeyModifiers::SHIFT | KeyModifiers::CONTROL,
                ) =>
            {
                if self.editing_branch {
                    self.editing_branch = false;
                    return PromptAction::None;
                }
                let branch = self.branch_name();
                return PromptAction::Launch(Launch {
                    project: self.project.clone(),
                    tree: self.tree.clone(),
                    branch,
                    model: self.model.clone(),
                    effort: self.effort.clone(),
                    prompt: self.text.text.trim().to_string(),
                });
            }
            _ => {
                if self.editing_branch {
                    let before = self.branch.text.clone();
                    self.branch.handle_key(&k);
                    if self.branch.text != before {
                        self.branch_edited = true;
                    }
                } else {
                    self.text.handle_key(&k);
                }
            }
        }
        PromptAction::None
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let w = (area.width * 6 / 10).clamp(50.min(area.width), 100);
        // Borders, the text area's side padding, and the "› " prefix.
        let inner_w = w.saturating_sub(6) as usize;
        let (lines, (crow, ccol)) = self.text.layout(inner_w);
        let text_h = (lines.len() as u16).clamp(3, 12);
        let h = text_h + 6;
        let r = Rect {
            x: area.x + (area.width.saturating_sub(w)) / 2,
            y: area.y + area.height.saturating_sub(h) / 3,
            width: w,
            height: h.min(area.height),
        };
        f.render_widget(Clear, r);
        let block = ui::modal_block(" New prompt ", t).title_bottom(Line::styled(
            " enter send · ⌥/⇧enter newline · esc cancel ",
            Style::default().fg(t.muted),
        ));
        let inner = block.inner(r);
        f.render_widget(block, r);

        let key = |s: &str| Span::styled(s.to_string(), Style::default().fg(t.muted));
        let val =
            |s: String| Span::styled(s, Style::default().fg(t.fg).add_modifier(Modifier::BOLD));
        let label = |s: &str| Span::styled(s.to_string(), Style::default().fg(t.muted));
        let project_name = self
            .projects
            .iter()
            .find(|p| p.path == self.project)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| tilde(&self.project));
        let mut row1 = vec![label(" project "), val(project_name), key(" ^p")];
        row1.push(label("  ·  tree "));
        match &self.tree {
            Tree::Main => row1.push(val("main checkout".into())),
            Tree::Existing { branch, .. } => row1.push(val(format!("⎇ {branch}"))),
            Tree::New => {
                let b = if self.editing_branch {
                    self.branch.text.clone()
                } else {
                    self.branch_name()
                };
                let st = if self.editing_branch {
                    Style::default().fg(t.green).bg(t.selection)
                } else {
                    Style::default().fg(t.green).add_modifier(Modifier::BOLD)
                };
                row1.push(Span::styled(format!("new ⎇ {b}"), st));
            }
        }
        row1.push(key(if self.tree == Tree::New {
            " ^t · tab rename"
        } else {
            " ^t"
        }));
        let effort = if self.effort.is_empty() {
            "default".to_string()
        } else {
            self.effort.clone()
        };
        let model = if self.model.is_empty() {
            "default".to_string()
        } else {
            self.model.clone()
        };
        let row2 = vec![
            label(" model "),
            val(model),
            label(" · effort "),
            val(effort),
            key(" ^o"),
        ];
        f.render_widget(
            Paragraph::new(vec![
                ui::truncate_line(Line::from(row1), inner.width as usize),
                Line::from(row2),
                Line::styled(
                    "─".repeat(inner.width as usize),
                    Style::default().fg(t.border),
                ),
            ]),
            Rect { height: 3, ..inner },
        );

        let text_area = Rect {
            x: inner.x + 1,
            y: inner.y + 3,
            width: inner.width.saturating_sub(2),
            height: text_h,
        };
        let scroll = crow.saturating_sub(text_h as usize - 1);
        let mut out: Vec<Line> = lines
            .iter()
            .skip(scroll)
            .take(text_h as usize)
            .enumerate()
            .map(|(i, l)| {
                let prefix = if i + scroll == 0 { "› " } else { "  " };
                Line::from(vec![
                    Span::styled(prefix, Style::default().fg(t.accent)),
                    Span::raw(l.clone()),
                ])
            })
            .collect();
        if self.text.text.is_empty() {
            out = vec![Line::from(vec![
                Span::styled("› ", Style::default().fg(t.accent)),
                Span::styled(
                    "What should Claude do? (empty starts a bare session)",
                    Style::default().fg(t.muted),
                ),
            ])];
        }
        f.render_widget(
            Paragraph::new(out).style(Style::default().fg(t.fg)),
            text_area,
        );
        if self.picker.is_none() {
            if self.editing_branch {
                // Cursor sits at end of the branch name on row 1; approximate position.
                let prefix: usize =
                    Line::from(row_prefix_width(&self.projects, &self.project)).width();
                let x = inner.x as usize
                    + prefix
                    + "new ⎇ ".chars().count()
                    + self.branch.text.chars().count();
                f.set_cursor_position(((x as u16).min(inner.x + inner.width - 1), inner.y));
            } else {
                f.set_cursor_position((
                    text_area.x + 2 + ccol as u16,
                    text_area.y + (crow - scroll) as u16,
                ));
            }
        }

        if let Some(p) = &self.picker {
            draw_picker(f, r, p, t, (p.title == "Model").then_some(&self.effort));
        }
    }
}

fn row_prefix_width(projects: &[ProjectEntry], project: &PathBuf) -> Vec<Span<'static>> {
    let name = projects
        .iter()
        .find(|p| &p.path == project)
        .map(|p| p.name.clone())
        .unwrap_or_else(|| tilde(project));
    vec![Span::raw(format!(" project {name} ^p  ·  tree "))]
}

fn draw_picker(f: &mut Frame, parent: Rect, p: &Picker, t: &Theme, effort: Option<&String>) {
    let w = (parent.width.saturating_sub(10)).clamp(30.min(parent.width), 70);
    let h = (p.shown.len() as u16 + 4).clamp(5, 16);
    let r = Rect {
        x: parent.x + (parent.width - w) / 2,
        y: parent.y + 1,
        width: w,
        height: h,
    };
    f.render_widget(Clear, r);
    let title = match effort {
        Some(e) => format!(
            " {} · effort {} (←/→) ",
            p.title,
            if e.is_empty() { "default" } else { e }
        ),
        None => format!(" {} ", p.title),
    };
    let block = ui::modal_block(&title, t);
    let inner = block.inner(r);
    f.render_widget(block, r);
    let mut lines = vec![Line::from(vec![
        Span::styled("› ", Style::default().fg(t.accent)),
        Span::styled(p.filter.text.clone(), Style::default().fg(t.fg)),
    ])];
    let lh = inner.height.saturating_sub(1) as usize;
    let off = p.sel.saturating_sub(lh.saturating_sub(1));
    for (i, &oi) in p.shown.iter().enumerate().skip(off).take(lh) {
        let (label, detail, _) = &p.options[oi];
        let base = if i == p.sel {
            Style::default().bg(t.selection)
        } else {
            Style::default()
        };
        let mut l = Line::from(vec![
            Span::styled(format!(" {label}"), base.fg(t.fg)),
            Span::styled(format!("  {detail}"), base.fg(t.muted)),
        ]);
        let used = l.width();
        l.spans.push(Span::styled(
            " ".repeat((inner.width as usize).saturating_sub(used)),
            base,
        ));
        lines.push(ui::truncate_line(l, inner.width as usize));
    }
    f.render_widget(Paragraph::new(lines), inner);
    f.set_cursor_position((inner.x + 2 + p.filter.text.chars().count() as u16, inner.y));
}
