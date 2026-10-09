use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
};

use crate::{
    app::App,
    session::Status,
    theme::Theme,
    ui::{self, status_dot, status_word},
};

/// Rows a tile takes, borders included.
const TILE_H: u16 = 5;
const TILE_MIN_W: u16 = 26;
const TILE_MAX_W: u16 = 34;

/// What one tile shows about a session.
pub struct Tile {
    pub id: String,
    pub name: String,
    pub project: String,
    /// The worktree branch, when the session runs in a linked worktree.
    pub branch: Option<String>,
    pub dot: String,
    pub color: Color,
    pub status: &'static str,
    pub age: String,
    pub state: Status,
    pub unseen: bool,
    /// Border colour for sessions that want a look: red for needs input, blue for unseen.
    pub attention: Option<Color>,
}

/// The agent list's sessions as tiles, in attention order.
pub fn tiles(app: &App) -> Vec<Tile> {
    let t = app.theme;
    app.attention_order()
        .iter()
        .filter_map(|id| app.session(id))
        .map(|s| {
            let (dot, color) = status_dot(s, app.tick, t);
            Tile {
                id: s.id.clone(),
                name: s.label().to_string(),
                project: app.project_name(&s.project),
                branch: (s.cwd != s.project)
                    .then(|| s.branch.clone().unwrap_or_else(|| "worktree".into())),
                dot,
                color,
                status: status_word(s),
                age: ui::age(s.created),
                state: s.status,
                unseen: s.status == Status::Done && !s.seen,
                attention: match s.status {
                    Status::NeedsInput => Some(t.red),
                    Status::Done if !s.seen => Some(t.blue),
                    _ => None,
                },
            }
        })
        .collect()
}

/// Columns and tile width for `n` tiles in `width` columns of room.
fn grid(n: usize, width: u16) -> (usize, u16) {
    let cols = (width / TILE_MIN_W).max(1) as usize;
    let cols = cols.min(n.max(1));
    let w = (width / cols as u16).min(TILE_MAX_W);
    (cols, w)
}

/// Moves `i` by `dx` columns and `dy` rows in a grid of `n` tiles, `cols` wide, staying put
/// at the edges.
fn step(i: usize, n: usize, cols: usize, dx: i32, dy: i32) -> usize {
    if n == 0 {
        return 0;
    }
    let (row, col) = ((i / cols) as i32, (i % cols) as i32);
    let rows = n.div_ceil(cols) as i32;
    let col = (col + dx).clamp(0, cols as i32 - 1);
    let row = (row + dy).clamp(0, rows - 1);
    ((row * cols as i32 + col) as usize).min(n - 1)
}

pub enum OverviewAction {
    None,
    Close,
    Open(String),
}

#[derive(Default)]
pub struct Overview {
    /// The selected session; it stays selected as the order changes.
    selected: Option<String>,
    /// First visible row of tiles.
    offset: usize,
    /// The order and column count last drawn, for moving between tiles.
    order: Vec<String>,
    cols: usize,
    /// Where each tile was last drawn, for clicks.
    hits: Vec<(Rect, String)>,
}

impl Overview {
    fn index(&self) -> usize {
        self.selected
            .as_ref()
            .and_then(|s| self.order.iter().position(|o| o == s))
            .unwrap_or(0)
    }

    fn move_by(&mut self, dx: i32, dy: i32) {
        let n = self.order.len();
        if n == 0 {
            return;
        }
        let i = step(self.index(), n, self.cols.max(1), dx, dy);
        self.selected = Some(self.order[i].clone());
    }

    fn select_at(&mut self, i: usize) {
        if let Some(id) = self.order.get(i) {
            self.selected = Some(id.clone());
        }
    }

    pub fn handle_key(&mut self, k: KeyEvent) -> OverviewAction {
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') => return OverviewAction::Close,
            KeyCode::Enter => {
                if let Some(id) = self.order.get(self.index()) {
                    return OverviewAction::Open(id.clone());
                }
            }
            KeyCode::Left | KeyCode::Char('h') => self.move_by(-1, 0),
            KeyCode::Right | KeyCode::Char('l') => self.move_by(1, 0),
            KeyCode::Up | KeyCode::Char('k') => self.move_by(0, -1),
            KeyCode::Down | KeyCode::Char('j') => self.move_by(0, 1),
            KeyCode::Tab => self.select_at((self.index() + 1) % self.order.len().max(1)),
            KeyCode::BackTab => {
                let n = self.order.len().max(1);
                self.select_at((self.index() + n - 1) % n);
            }
            KeyCode::Home | KeyCode::Char('g') => self.select_at(0),
            KeyCode::End | KeyCode::Char('G') => self.select_at(self.order.len().saturating_sub(1)),
            _ => {}
        }
        OverviewAction::None
    }

    pub fn handle_mouse(&mut self, m: MouseEvent) -> OverviewAction {
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let hit = self.hits.iter().find(|(r, _)| {
                    m.column >= r.x
                        && m.column < r.x + r.width
                        && m.row >= r.y
                        && m.row < r.y + r.height
                });
                if let Some((_, id)) = hit {
                    return OverviewAction::Open(id.clone());
                }
            }
            MouseEventKind::ScrollDown => self.move_by(0, 1),
            MouseEventKind::ScrollUp => self.move_by(0, -1),
            _ => {}
        }
        OverviewAction::None
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, tiles: &[Tile], close: &str, t: &Theme) {
        let max_w = (area.width * 9 / 10).max(TILE_MIN_W + 4).min(area.width);
        // A column of margin each side, inside the border.
        let (cols, tile_w) = grid(tiles.len(), max_w.saturating_sub(4));
        let rows = tiles.len().div_ceil(cols).max(1);
        // Border, a summary line and a gap above the tiles.
        let room = area.height.saturating_sub(2).max(TILE_H + 4);
        let visible = ((room - 4) / TILE_H).max(1) as usize;
        let w = (cols as u16 * tile_w + 4).clamp(56.min(area.width), area.width);
        let h = (visible.min(rows) as u16 * TILE_H + 4).min(area.height);
        let r = Rect {
            x: area.x + (area.width - w) / 2,
            y: area.y + (area.height - h) / 2,
            width: w,
            height: h,
        };

        self.order = tiles.iter().map(|t| t.id.clone()).collect();
        self.cols = cols;
        if self
            .selected
            .as_ref()
            .is_none_or(|s| !self.order.contains(s))
        {
            self.selected = self.order.first().cloned();
        }
        let sel = self.index();
        let sel_row = sel / cols;
        if sel_row < self.offset {
            self.offset = sel_row;
        } else if sel_row >= self.offset + visible {
            self.offset = sel_row + 1 - visible;
        }
        self.offset = self.offset.min(rows.saturating_sub(visible));

        f.render_widget(Clear, r);
        let key = Style::default().fg(t.accent).add_modifier(Modifier::BOLD);
        let muted = Style::default().fg(t.muted);
        let mut title = format!(" Agents ({}) ", tiles.len());
        if rows > visible {
            let last = (self.offset + visible).min(rows);
            title = format!(
                " Agents ({}) · rows {}–{last} of {rows} ",
                tiles.len(),
                self.offset + 1
            );
        }
        let block = ui::modal_block(&title, t).title_bottom(Line::from(vec![
            Span::styled(" ←↑↓→", key),
            Span::styled(" move  ", muted),
            Span::styled("⏎", key),
            Span::styled(" open  ", muted),
            Span::styled("esc", key),
            Span::styled(" / ", muted),
            Span::styled(close.to_string(), key),
            Span::styled(" close ", muted),
        ]));
        let inner = block.inner(r);
        f.render_widget(block, r);

        if tiles.is_empty() {
            f.render_widget(
                Paragraph::new(Line::styled(
                    " No agents yet — start one with a new prompt.",
                    muted,
                )),
                inner,
            );
            self.hits.clear();
            return;
        }

        let summary = summary(tiles, t);
        f.render_widget(
            Paragraph::new(ui::truncate_line(summary, inner.width as usize)),
            inner,
        );

        self.hits.clear();
        let x0 = inner.x + inner.width.saturating_sub(cols as u16 * tile_w) / 2;
        for (i, tile) in tiles
            .iter()
            .enumerate()
            .skip(self.offset * cols)
            .take(visible * cols)
        {
            let (row, col) = (i / cols - self.offset, i % cols);
            let tr = Rect {
                x: x0 + col as u16 * tile_w,
                y: inner.y + 2 + row as u16 * TILE_H,
                width: tile_w,
                height: TILE_H,
            }
            .intersection(inner);
            draw_tile(f, tr, tile, i == sel, t);
            self.hits.push((tr, tile.id.clone()));
        }
    }
}

/// Counts by status, e.g. `● 2 need input  ● 1 working  ○ 3 idle  ◌ 1 stopped`.
fn summary(tiles: &[Tile], t: &Theme) -> Line<'static> {
    let count = |f: &dyn Fn(&Tile) -> bool| tiles.iter().filter(|x| f(x)).count();
    let need = count(&|x| x.state == Status::NeedsInput);
    let working = count(&|x| x.state == Status::Working);
    let unseen = count(&|x| x.unseen);
    let stopped = count(&|x| x.state == Status::Stopped);
    let idle = tiles.len() - need - working - unseen - stopped;
    let mut spans = vec![Span::raw(" ")];
    for (n, dot, what, color) in [
        (
            need,
            "●",
            if need == 1 {
                "needs input"
            } else {
                "need input"
            },
            t.red,
        ),
        (working, "●", "working", t.yellow),
        (unseen, "●", "unseen", t.blue),
        (idle, "○", "idle", t.muted),
        (stopped, "◌", "stopped", t.border),
    ] {
        if n > 0 {
            spans.push(Span::styled(format!("{dot} "), Style::default().fg(color)));
            spans.push(Span::styled(
                format!("{n} {what}   "),
                Style::default().fg(if color == t.border { t.muted } else { color }),
            ));
        }
    }
    Line::from(spans)
}

fn draw_tile(f: &mut Frame, r: Rect, tile: &Tile, selected: bool, t: &Theme) {
    let border = if selected {
        t.accent
    } else {
        tile.attention.unwrap_or(t.border)
    };
    let bg = if selected { t.selection } else { t.surface };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(if selected {
            BorderType::Thick
        } else {
            BorderType::Rounded
        })
        .border_style(Style::default().fg(border))
        .style(Style::default().bg(bg).fg(t.fg));
    let inner = block.inner(r);
    f.render_widget(block, r);
    let width = inner.width as usize;

    let name_style = if tile.state == Status::Stopped {
        Style::default().fg(t.muted)
    } else {
        Style::default().fg(t.fg).add_modifier(Modifier::BOLD)
    };
    let name = Line::from(vec![
        Span::styled(format!(" {} ", tile.dot), Style::default().fg(tile.color)),
        Span::styled(tile.name.clone(), name_style),
    ]);
    let mut place = vec![Span::styled(
        format!("   {}", tile.project),
        Style::default().fg(t.muted),
    )];
    if let Some(b) = &tile.branch {
        place.push(Span::styled(
            format!(" ⎇ {b}"),
            Style::default().fg(t.green),
        ));
    }
    let status_color = if tile.state == Status::Stopped {
        t.muted
    } else {
        tile.color
    };
    let status = Span::styled(
        format!("   {}", tile.status),
        Style::default().fg(status_color),
    );
    let age = format!("{} ", tile.age);
    let pad = width.saturating_sub(status.width() + age.chars().count());
    let status = ui::truncate_line(
        Line::from(vec![
            status,
            Span::raw(" ".repeat(pad)),
            Span::styled(age, Style::default().fg(t.muted)),
        ]),
        width,
    );
    f.render_widget(
        Paragraph::new(vec![
            ui::truncate_line(name, width),
            ui::truncate_line(Line::from(place), width),
            status,
        ]),
        inner,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_tiles_to_the_width() {
        assert_eq!(grid(0, 100), (1, TILE_MAX_W));
        assert_eq!(grid(2, 200), (2, TILE_MAX_W));
        assert_eq!(grid(10, 100), (3, 33));
        assert_eq!(grid(10, 20), (1, 20));
    }

    #[test]
    fn moves_within_the_grid() {
        // 7 tiles, 3 wide:  0 1 2 / 3 4 5 / 6
        assert_eq!(step(0, 7, 3, 1, 0), 1);
        assert_eq!(step(2, 7, 3, 1, 0), 2);
        assert_eq!(step(0, 7, 3, -1, 0), 0);
        assert_eq!(step(1, 7, 3, 0, 1), 4);
        assert_eq!(step(4, 7, 3, 0, 1), 6);
        assert_eq!(step(6, 7, 3, 0, 1), 6);
        assert_eq!(step(6, 7, 3, 0, -1), 3);
        assert_eq!(step(0, 0, 3, 1, 1), 0);
    }
}
