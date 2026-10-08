use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    app::{App, Focus, GraphLine, Modal, RowKey},
    session::{Session, Status},
    theme::Theme,
};

const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn fill(bg: Color) -> Block<'static> {
    Block::default().style(Style::default().bg(bg))
}

pub fn panel<'a>(title: &str, focused: bool, t: &Theme) -> Block<'a> {
    let border = if focused { t.accent } else { t.border };
    let title_style = if focused {
        Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(t.muted)
    };
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border))
        .title(Span::styled(title.to_string(), title_style))
        .style(Style::default().bg(t.bg).fg(t.fg))
}

pub fn modal_block<'a>(title: &str, t: &Theme) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.accent))
        .title(Span::styled(
            title.to_string(),
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(t.surface).fg(t.fg))
}

pub fn centered(area: Rect, pct_w: u16, pct_h: u16) -> Rect {
    let w = area.width * pct_w / 100;
    let h = area.height * pct_h / 100;
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

/// Cut a line to `width` display columns, adding an ellipsis when it overflows.
pub fn truncate_line(line: Line<'static>, width: usize) -> Line<'static> {
    if line.width() <= width {
        return line;
    }
    let style = line.style;
    let mut out = Vec::new();
    let mut used = 0;
    for span in line.spans {
        let mut s = String::new();
        for ch in span.content.chars() {
            let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            if used + w + 1 > width {
                out.push(Span::styled(s, span.style));
                out.push(Span::styled("…", span.style));
                return Line::from(out).style(style);
            }
            s.push(ch);
            used += w;
        }
        out.push(Span::styled(s, span.style));
    }
    Line::from(out).style(style)
}

pub fn status_dot(s: &Session, tick: u64, t: &Theme) -> (String, Color) {
    match s.status {
        Status::Working => (SPINNER[(tick as usize) % SPINNER.len()].into(), t.yellow),
        Status::NeedsInput => ("●".into(), t.red),
        Status::Done if !s.seen => ("●".into(), t.blue),
        Status::Done | Status::Idle => ("○".into(), t.muted),
        Status::Stopped => ("◌".into(), t.border),
    }
}

pub fn status_word(s: &Session) -> &'static str {
    match s.status {
        Status::Working => "working",
        Status::NeedsInput => "needs input",
        Status::Done if !s.seen => "done · unseen",
        Status::Done => "done",
        Status::Idle => "idle",
        Status::Stopped => "stopped",
    }
}

fn age(secs: u64) -> String {
    let d = crate::session::now_secs().saturating_sub(secs);
    match d {
        0..60 => "now".into(),
        60..3600 => format!("{}m", d / 60),
        3600..86400 => format!("{}h", d / 3600),
        _ => format!("{}d", d / 86400),
    }
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let t = app.theme;
    let area = f.area();
    f.render_widget(fill(t.bg), area);
    let main = Rect {
        height: area.height.saturating_sub(1),
        ..area
    };
    let lw = app
        .state
        .list_width
        .clamp(18, main.width.saturating_sub(30).max(18));
    app.list_rect = Rect {
        width: lw.min(main.width),
        ..main
    };
    app.pane_rect = Rect {
        x: main.x + lw,
        width: main.width.saturating_sub(lw),
        ..main
    };

    // The graph takes the bottom of the left column when open and there's room for it.
    let tl_h = graph_height(app.state.graph_height, main.height);
    let (agents, graph) = if app.state.graph_open && main.height >= 16 {
        let r = app.list_rect;
        (
            Rect {
                height: r.height - tl_h,
                ..r
            },
            Some(Rect {
                y: r.y + r.height - tl_h,
                height: tl_h,
                ..r
            }),
        )
    } else {
        (app.list_rect, None)
    };
    app.graph_rect = graph.unwrap_or_default();
    draw_list(f, app, agents);
    if let Some(r) = graph {
        draw_graph(f, app, r);
    }
    draw_pane(f, app);
    draw_footer(
        f,
        app,
        Rect {
            y: area.y + area.height.saturating_sub(1),
            height: 1,
            ..area
        },
    );

    let theme = app.theme;
    match &mut app.modal {
        Some(Modal::Switcher(s)) => s.draw(f, area, theme),
        Some(Modal::Prompt(p)) => p.draw(f, area, theme),
        Some(Modal::Diff(d)) => d.draw(f, area, theme),
        Some(Modal::Settings(s)) => s.draw(f, area, &app.cfg, theme),
        Some(Modal::Confirm(c)) => {
            let w = (c.message.chars().count() as u16 + 6).clamp(30, area.width);
            let r = Rect {
                x: area.x + (area.width - w) / 2,
                y: area.y + area.height / 3,
                width: w,
                height: 5,
            };
            f.render_widget(Clear, r);
            let block = modal_block(" Confirm ", theme);
            let inner = block.inner(r);
            f.render_widget(block, r);
            f.render_widget(
                Paragraph::new(vec![
                    Line::raw(format!(" {}", c.message)),
                    Line::raw(""),
                    Line::from(vec![
                        Span::styled(
                            " y",
                            Style::default()
                                .fg(theme.accent)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(" yes   ", Style::default().fg(theme.muted)),
                        Span::styled(
                            "n",
                            Style::default()
                                .fg(theme.accent)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(" no", Style::default().fg(theme.muted)),
                    ]),
                ]),
                inner,
            );
        }
        Some(Modal::Rename(rn)) => {
            let w = (area.width * 6 / 10).clamp(30, area.width);
            let r = Rect {
                x: area.x + (area.width - w) / 2,
                y: area.y + area.height / 3,
                width: w,
                height: 5,
            };
            f.render_widget(Clear, r);
            let block = modal_block(" Rename session ", theme);
            let inner = block.inner(r);
            f.render_widget(block, r);
            let (lines, (_, ccol)) = rn.input.layout(inner.width.saturating_sub(4) as usize);
            let text = lines.last().cloned().unwrap_or_default();
            let key = |s: &'static str| {
                Span::styled(
                    s,
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                )
            };
            let txt = |s: &'static str| Span::styled(s, Style::default().fg(theme.muted));
            f.render_widget(
                Paragraph::new(vec![
                    Line::from(vec![
                        Span::styled(" › ", Style::default().fg(theme.accent)),
                        Span::styled(text, Style::default().fg(theme.fg)),
                    ]),
                    Line::raw(""),
                    Line::from(vec![key(" ⏎"), txt(" save   "), key("esc"), txt(" cancel")]),
                ]),
                inner,
            );
            f.set_cursor_position((inner.x + 3 + ccol as u16, inner.y));
        }
        None => {}
    }
}

/// Graph panel height for a left column of `column` rows: the saved height (0 = automatic),
/// kept between 4 rows and enough to leave the Agents panel 6.
pub fn graph_height(saved: u16, column: u16) -> u16 {
    let h = if saved == 0 {
        (column * 2 / 5).min(14)
    } else {
        saved
    };
    h.clamp(4, column.saturating_sub(6).max(4))
}

fn draw_list(f: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme;
    let focused = app.focus == Focus::List && app.modal.is_none();
    let block = panel(" Agents ", focused, t);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let rows = app.rows();
    let sel_idx = app
        .selected
        .as_ref()
        .and_then(|s| rows.iter().position(|r| r == s));
    let h = inner.height as usize;
    if let Some(i) = sel_idx {
        if i < app.list_offset {
            app.list_offset = i;
        } else if h > 0 && i >= app.list_offset + h {
            app.list_offset = i + 1 - h;
        }
    }
    app.list_offset = app.list_offset.min(rows.len().saturating_sub(1));
    app.list_rows.clear();
    let width = inner.width as usize;
    let mut lines = Vec::new();
    if rows.is_empty() {
        lines.push(Line::styled(
            " No projects yet.",
            Style::default().fg(t.muted),
        ));
        lines.push(Line::styled(
            format!(" {} → Add folder", app.keys.switcher.short()),
            Style::default().fg(t.muted),
        ));
    }
    for (i, row) in rows.iter().enumerate().skip(app.list_offset).take(h) {
        let selected = Some(i) == sel_idx;
        let base = if selected {
            Style::default().bg(t.selection)
        } else {
            Style::default()
        };
        app.list_rows
            .push((inner.y + (i - app.list_offset) as u16, row.clone()));
        let mut spans: Vec<Span> = Vec::new();

        let right: Vec<Span> = match row {
            RowKey::Project(path) => {
                let name = app.project_name(path);
                let sessions: Vec<&Session> = app.project_sessions(path);
                let any_live = sessions.iter().any(|s| s.status.is_live());
                let dot = if any_live {
                    ("●", t.accent)
                } else {
                    ("○", t.muted)
                };
                spans.push(Span::styled(format!(" {} ", dot.0), base.fg(dot.1)));
                spans.push(Span::styled(
                    name,
                    base.fg(t.fg).add_modifier(Modifier::BOLD),
                ));
                let count = |st: fn(&Session) -> bool| sessions.iter().filter(|s| st(s)).count();
                let mut r = Vec::new();
                for (n, c) in [
                    (count(|s| s.status == Status::NeedsInput), t.red),
                    (count(|s| s.status == Status::Working), t.yellow),
                    (count(|s| s.status == Status::Done && !s.seen), t.blue),
                ] {
                    if n > 0 {
                        r.push(Span::styled(format!(" ●{n}"), base.fg(c)));
                    }
                }
                r.push(Span::styled(" ", base));
                r
            }
            RowKey::Session(id) => {
                let Some(s) = app.session(id) else { continue };
                let (dot, color) = status_dot(s, app.tick, t);
                spans.push(Span::styled(format!("   {dot} "), base.fg(color)));
                let name_style = if s.status == Status::Stopped {
                    base.fg(t.muted)
                } else {
                    base.fg(t.fg)
                };
                let name = if s.name.is_empty() {
                    "new session".to_string()
                } else {
                    s.name.clone()
                };
                spans.push(Span::styled(name, name_style));
                let mut r = Vec::new();
                if s.cwd != s.project {
                    r.push(Span::styled(" ⎇", base.fg(t.green)));
                }
                r.push(Span::styled(
                    format!(" {} ", age(s.created)),
                    base.fg(t.muted),
                ));
                r
            }
        };
        let rw: usize = right.iter().map(|s| s.width()).sum();
        let left = truncate_line(Line::from(spans), width.saturating_sub(rw));
        let lw = left.width();
        let mut all = left.spans;
        all.push(Span::styled(
            " ".repeat(width.saturating_sub(lw + rw)),
            base,
        ));
        all.extend(right);
        lines.push(Line::from(all));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_graph(f: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme;
    let focused = app.focus == Focus::Graph && app.modal.is_none();
    let block = panel(" Graph ", focused, t);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let g = &mut app.graph;
    let muted = Style::default().fg(t.muted);
    let empty = if g.dir.is_none() {
        Some(" No project selected")
    } else if !g.loaded {
        Some(" Loading…")
    } else if g.rows.is_empty() {
        Some(" No commits")
    } else {
        None
    };
    if let Some(msg) = empty {
        f.render_widget(Paragraph::new(Line::styled(msg, muted)), inner);
        return;
    }
    let all = g.lines();
    let cursor = g.cursor();
    let at = all.iter().position(|l| *l == cursor).unwrap_or(0);
    let h = inner.height as usize;
    if at < g.offset {
        g.offset = at;
    } else if h > 0 && at >= g.offset + h {
        g.offset = at + 1 - h;
    }
    g.offset = g.offset.min(all.len().saturating_sub(1));
    let width = inner.width as usize;
    let lines: Vec<Line> = all
        .iter()
        .skip(g.offset)
        .take(h)
        .map(|&line| {
            let base = if focused && line == cursor {
                Style::default().bg(t.selection)
            } else {
                Style::default()
            };
            let i = match line {
                GraphLine::Row(i) => i,
                GraphLine::File(fi) => {
                    let ch = &g.files[fi];
                    let (letter, color) = match ch.status {
                        'A' => ('A', t.green),
                        'D' => ('D', t.red),
                        'R' => ('R', t.blue),
                        _ => ('M', t.yellow),
                    };
                    // Continue the commit's graph lane down through its files.
                    let lane: String = g.rows[g.sel]
                        .graph
                        .chars()
                        .map(|c| if c == '*' { '│' } else { c })
                        .collect();
                    let line = truncate_line(
                        Line::from(vec![
                            Span::styled(format!(" {lane}  "), base.fg(t.border)),
                            Span::styled(format!("{letter} "), base.fg(color)),
                            Span::styled(ch.path.clone(), base.fg(t.fg)),
                        ]),
                        width,
                    );
                    let pad = width.saturating_sub(line.width());
                    let mut spans = line.spans;
                    spans.push(Span::styled(" ".repeat(pad), base));
                    return Line::from(spans);
                }
            };
            let row = &g.rows[i];
            let mut spans = vec![Span::styled(" ", base)];
            for ch in row.graph.chars() {
                let (s, c) = if ch == '*' {
                    ('●', t.accent)
                } else {
                    (ch, t.muted)
                };
                spans.push(Span::styled(s.to_string(), base.fg(c)));
            }
            let Some(c) = &row.commit else {
                return Line::from(spans);
            };
            spans.push(Span::styled(format!(" {} ", c.hash), base.fg(t.muted)));
            if !c.refs.is_empty() {
                spans.push(Span::styled(format!("({}) ", c.refs), base.fg(t.green)));
            }
            spans.push(Span::styled(c.subject.clone(), base.fg(t.fg)));
            let right = Span::styled(format!(" {} ", age(c.time)), base.fg(t.muted));
            let left = truncate_line(Line::from(spans), width.saturating_sub(right.width()));
            let pad = width.saturating_sub(left.width() + right.width());
            let mut spans = left.spans;
            spans.push(Span::styled(" ".repeat(pad), base));
            spans.push(right);
            Line::from(spans)
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

/// The empty pane: a hint for the next step, then the most useful shortcuts.
fn draw_help(f: &mut Frame, app: &App, area: Rect, msg: String) {
    let t = app.theme;
    let k = &app.keys;
    let shortcuts: Vec<(String, &str)> = vec![
        (k.new_prompt.short(), "New prompt"),
        ("w".into(), "New prompt in a new worktree"),
        (k.switcher.short(), "Switch sessions, projects, commands"),
        (k.next_session.short(), "Jump to the session that needs you"),
        (
            format!("{} {}", k.focus_list.short(), k.focus_pane.short()),
            "Focus agent list / pane",
        ),
        (k.diff.short(), "View uncommitted changes"),
        (k.graph.short(), "Show/hide the commit graph"),
        ("e".into(), "Rename the selected session"),
        ("x".into(), "Close session / remove project"),
        (k.settings.short(), "Settings and key bindings"),
    ];
    // Message, blank line, then as many shortcuts as fit.
    let room = (area.height as usize).saturating_sub(2);
    let shortcuts = &shortcuts[..shortcuts.len().min(room)];
    let kw = shortcuts.iter().map(|(k, _)| k.width()).max().unwrap_or(0);
    let dw = shortcuts.iter().map(|(_, d)| d.width()).max().unwrap_or(0);
    let pad = (area.width as usize).saturating_sub(kw + 2 + dw) / 2;
    let mut lines = vec![
        Line::styled(msg, Style::default().fg(t.fg)).centered(),
        Line::raw(""),
    ];
    for (key, desc) in shortcuts {
        lines.push(Line::from(vec![
            Span::raw(" ".repeat(pad + kw - key.width())),
            Span::styled(
                key.clone(),
                Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {desc}"), Style::default().fg(t.muted)),
        ]));
    }
    let h = (lines.len() as u16).min(area.height);
    f.render_widget(
        Paragraph::new(lines),
        Rect {
            y: area.y + (area.height - h) / 2,
            height: h,
            ..area
        },
    );
}

fn vt_color(c: vt100::Color, default: Color) -> Color {
    match c {
        vt100::Color::Default => default,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

fn draw_pane(f: &mut Frame, app: &mut App) {
    let t = app.theme;
    // A file picked in the commit graph takes over the pane while the graph has focus.
    if app.focus == Focus::Graph
        && let Some(p) = &mut app.graph.preview
    {
        p.draw(f, app.pane_rect, false, t);
        return;
    }
    let focused = app.focus == Focus::Pane && app.modal.is_none();
    let current = app.current.clone().and_then(|id| app.session_idx(&id));
    let title = match current {
        Some(i) => {
            let s = &app.sessions[i];
            let branch = s.branch.clone().unwrap_or_else(|| "–".into());
            let model = if s.model.is_empty() {
                "default".to_string()
            } else {
                s.model.clone()
            };
            format!(
                " {} · {} · {} · {} ",
                app.project_name(&s.project),
                model,
                branch,
                status_word(s)
            )
        }
        None => " ".into(),
    };
    let block = panel(&title, focused, t);
    let inner = block.inner(app.pane_rect);
    f.render_widget(block, app.pane_rect);
    app.pane_inner = inner;

    let Some(i) = current else {
        let project = app.selected_project();
        let msg = match project {
            Some(p) => format!(
                "No session selected. {} starts one in {}.",
                app.keys.new_prompt.short(),
                app.project_name(&p)
            ),
            None => format!(
                "{} to add a folder, then {} to start a prompt.",
                app.keys.switcher.short(),
                app.keys.new_prompt.short()
            ),
        };
        draw_help(f, app, inner, msg);
        return;
    };

    // Keep the pty sized to the pane.
    app.sessions[i].resize(inner.height, inner.width);
    let s = &mut app.sessions[i];
    let mut parser = s.parser.lock().unwrap();
    parser.set_scrollback(s.scroll);
    s.scroll = parser.screen().scrollback();
    let screen = parser.screen();
    let sel = app.selection.map(|(a, b)| {
        if (a.1, a.0) <= (b.1, b.0) {
            (a, b)
        } else {
            (b, a)
        }
    });
    let in_sel = |row: u16, col: u16| {
        sel.is_some_and(|((sc, sr), (ec, er))| (row, col) >= (sr, sc) && (row, col) <= (er, ec))
    };
    let buf = f.buffer_mut();
    let (rows, cols) = screen.size();
    for row in 0..inner.height.min(rows) {
        for col in 0..inner.width.min(cols) {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            if cell.is_wide_continuation() {
                continue;
            }
            let mut fg = vt_color(cell.fgcolor(), t.fg);
            let mut bg = vt_color(cell.bgcolor(), t.bg);
            if cell.inverse() {
                std::mem::swap(&mut fg, &mut bg);
            }
            let mut style = Style::default().fg(fg).bg(bg);
            if cell.bold() {
                style = style.add_modifier(Modifier::BOLD);
            }
            if cell.italic() {
                style = style.add_modifier(Modifier::ITALIC);
            }
            if cell.underline() {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
            if in_sel(row, col) {
                style = style.bg(t.selection).fg(t.fg);
            }
            let contents = cell.contents();
            let symbol = if contents.is_empty() {
                " "
            } else {
                contents.as_str()
            };
            if let Some(c) = buf.cell_mut((inner.x + col, inner.y + row)) {
                c.set_symbol(symbol).set_style(style);
            }
        }
    }
    let scrolled = s.scroll;
    let live = s.pty.is_some();
    if focused && live && scrolled == 0 && !screen.hide_cursor() {
        let (cr, cc) = screen.cursor_position();
        if cr < inner.height && cc < inner.width {
            f.set_cursor_position((inner.x + cc, inner.y + cr));
        }
    }
    drop(parser);
    if scrolled > 0 {
        let tag = format!(" ↑ {scrolled} lines · ⇧PgDn / scroll to return ");
        let w = tag.chars().count() as u16;
        let r = Rect {
            x: inner.x + inner.width.saturating_sub(w),
            y: inner.y,
            width: w.min(inner.width),
            height: 1,
        };
        f.render_widget(
            Paragraph::new(tag).style(Style::default().bg(t.accent).fg(t.bg)),
            r,
        );
    }
    if !live {
        let msg = " Session stopped · enter or r to resume ";
        let w = (msg.chars().count() as u16).min(inner.width);
        let r = Rect {
            x: inner.x + (inner.width - w) / 2,
            y: inner.y + inner.height / 2,
            width: w,
            height: 1,
        };
        f.render_widget(Clear, r);
        f.render_widget(
            Paragraph::new(msg).style(Style::default().bg(t.surface).fg(t.yellow)),
            r,
        );
    }
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let t = app.theme;
    let k = &app.keys;
    let key = |s: String| Span::styled(s, Style::default().fg(t.accent));
    let txt = |s: &str| Span::styled(s.to_string(), Style::default().fg(t.muted));
    let mut spans = vec![Span::raw(" ")];
    let mut add = |kk: String, label: &str| {
        spans.push(key(kk));
        spans.push(txt(&format!(" {label}  ")));
    };
    add(k.switcher.short(), "switch");
    add(k.new_prompt.short(), "new");
    add(k.diff.short(), "diff");
    add(k.settings.short(), "settings");
    if app.focus == Focus::List {
        add("⏎".into(), "open");
        add("x".into(), "close");
        if app.state.graph_open {
            add(k.next_session.short(), "graph");
        }
        add("q".into(), "quit");
    } else if app.focus == Focus::Graph {
        add("j/k".into(), "move");
        if app.graph.file.is_some() {
            add("PgUp/PgDn".into(), "scroll");
            add("h/l".into(), "pan");
            add("s".into(), "split");
        } else {
            add("⏎".into(), "files");
            add("y".into(), "copy hash");
        }
        if app.graph.expanded() {
            add("esc".into(), "collapse");
        } else {
            add(k.prev_session.short(), "agents");
        }
    } else {
        add(k.focus_list.short(), "list");
        add(k.next_session.short(), "next");
        add("⇧PgUp".into(), "scroll");
    }
    let mut right: Vec<Span> = Vec::new();
    if let Some((msg, _)) = &app.toast {
        right.push(Span::styled(
            format!("{msg} "),
            Style::default().fg(t.yellow),
        ));
    } else {
        let waiting = app
            .sessions
            .iter()
            .filter(|s| s.status == Status::NeedsInput)
            .count();
        let working = app
            .sessions
            .iter()
            .filter(|s| s.status == Status::Working)
            .count();
        if waiting > 0 {
            right.push(Span::styled(
                format!("●{waiting} waiting  "),
                Style::default().fg(t.red),
            ));
        }
        if working > 0 {
            right.push(Span::styled(
                format!("●{working} working "),
                Style::default().fg(t.yellow),
            ));
        }
    }
    let rw: usize = right.iter().map(|s| s.width()).sum();
    let left = truncate_line(
        Line::from(spans),
        (area.width as usize).saturating_sub(rw + 1),
    );
    let lw = left.width();
    let mut all = left.spans;
    all.push(Span::raw(
        " ".repeat((area.width as usize).saturating_sub(lw + rw)),
    ));
    all.extend(right);
    f.render_widget(Paragraph::new(Line::from(all)), area);
}
