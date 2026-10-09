//! The shimmering wordmark shown for a moment while maximus starts.

use std::time::{Duration, Instant};

use anyhow::Result;
use crossbeam_channel::{Receiver, RecvTimeoutError};
use crossterm::event::{Event, KeyEventKind, MouseEventKind};
use ratatui::{Frame, Terminal, backend::Backend, style::Color};

use crate::{
    app::{App, AppEvent},
    theme::Theme,
    ui,
};

const DURATION: Duration = Duration::from_millis(1600);
const FRAME: Duration = Duration::from_millis(16);
/// Share of the intro spent fading in, and again fading out.
const FADE: f32 = 0.15;
/// Half-width of the band of light, in columns.
const BAND: f32 = 7.0;
/// How far each row down shifts the band, giving it a diagonal slant.
const SLANT: f32 = 2.0;

// ANSI Shadow letters, each six rows tall.
const M: [&str; 6] = [
    "███╗   ███╗",
    "████╗ ████║",
    "██╔████╔██║",
    "██║╚██╔╝██║",
    "██║ ╚═╝ ██║",
    "╚═╝     ╚═╝",
];
const A: [&str; 6] = [
    " █████╗ ",
    "██╔══██╗",
    "███████║",
    "██╔══██║",
    "██║  ██║",
    "╚═╝  ╚═╝",
];
const X: [&str; 6] = [
    "██╗  ██╗",
    "╚██╗██╔╝",
    " ╚███╔╝ ",
    " ██╔██╗ ",
    "██╔╝ ██╗",
    "╚═╝  ╚═╝",
];
const I: [&str; 6] = ["██╗", "██║", "██║", "██║", "██║", "╚═╝"];
const U: [&str; 6] = [
    "██╗   ██╗",
    "██║   ██║",
    "██║   ██║",
    "██║   ██║",
    "╚██████╔╝",
    " ╚═════╝ ",
];
const S: [&str; 6] = [
    "███████╗",
    "██╔════╝",
    "███████╗",
    "╚════██║",
    "███████║",
    "╚══════╝",
];
const WORD: [[&str; 6]; 7] = [M, A, X, I, M, U, S];

fn art() -> Vec<Vec<char>> {
    (0..6)
        .map(|row| WORD.iter().flat_map(|l| l[row].chars()).collect())
        .collect()
}

/// Play the intro until it finishes or a key or click skips it. Events that arrive meanwhile
/// still reach the app, so hooks and resizes aren't lost.
pub fn play<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    rx: &Receiver<AppEvent>,
) -> Result<()> {
    let art = art();
    let start = Instant::now();
    loop {
        let p = start.elapsed().as_secs_f32() / DURATION.as_secs_f32();
        if p >= 1.0 {
            return Ok(());
        }
        terminal.draw(|f| draw(f, app.theme, &art, p))?;
        match rx.recv_timeout(FRAME) {
            Ok(AppEvent::Input(Event::Key(k))) if k.kind == KeyEventKind::Press => return Ok(()),
            Ok(AppEvent::Input(Event::Mouse(m))) if matches!(m.kind, MouseEventKind::Down(_)) => {
                return Ok(());
            }
            Ok(ev) => app.handle(ev),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
        if app.quit {
            return Ok(());
        }
    }
}

fn draw(f: &mut Frame, t: &Theme, art: &[Vec<char>], p: f32) {
    let area = f.area();
    f.render_widget(ui::fill(t.bg), area);

    let fallback = [String::from("maximus").chars().collect::<Vec<_>>()];
    let art_w = art[0].len() as u16;
    let lines = if area.width >= art_w + 2 && area.height >= art.len() as u16 + 3 {
        art
    } else {
        &fallback[..]
    };
    let w = lines[0].len() as u16;
    let h = lines.len() as u16;
    let tagline = format!(
        "Claude Code, across every project · v{}",
        env!("CARGO_PKG_VERSION")
    );
    let block_h = h + 2;
    let x0 = area.x + area.width.saturating_sub(w) / 2;
    let y0 = area.y + area.height.saturating_sub(block_h) / 2;

    let alpha = (p / FADE).min((1.0 - p) / FADE).clamp(0.0, 1.0);
    // The band sweeps from fully off the left edge to fully off the right.
    let sweep = ((p - FADE * 0.5) / (1.0 - FADE * 1.5)).clamp(0.0, 1.0);
    let reach = w as f32 + h as f32 * SLANT + BAND * 2.0;
    let center = sweep * reach - BAND;
    let dark = luma(t.bg) < 0.5;
    let shine = if dark {
        Color::Rgb(255, 255, 255)
    } else {
        mix(t.bg, Color::Rgb(255, 255, 255), 0.6)
    };

    let buf = f.buffer_mut();
    for (row, chars) in lines.iter().enumerate() {
        for (col, &ch) in chars.iter().enumerate() {
            if ch == ' ' {
                continue;
            }
            let x = x0 + col as u16;
            let y = y0 + row as u16;
            let mut fg = mix(t.accent, t.blue, col as f32 / w.max(1) as f32);
            if ch != '█' {
                fg = mix(fg, t.bg, 0.45);
            }
            let phase = col as f32 + row as f32 * SLANT;
            let glow = smooth(1.0 - (phase - center).abs() / BAND);
            fg = mix(fg, shine, glow * 0.85);
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_char(ch).set_fg(mix(t.bg, fg, alpha)).set_bg(t.bg);
            }
        }
    }

    // The tagline types itself out over the first half.
    let shown = ((p * 2.0).min(1.0) * tagline.chars().count() as f32) as usize;
    let tw = tagline.chars().count() as u16;
    let tx = area.x + area.width.saturating_sub(tw) / 2;
    let ty = y0 + h + 1;
    if ty < area.y + area.height {
        let fg = mix(t.bg, t.muted, alpha);
        for (i, ch) in tagline.chars().take(shown).enumerate() {
            if let Some(cell) = buf.cell_mut((tx + i as u16, ty)) {
                cell.set_char(ch).set_fg(fg).set_bg(t.bg);
            }
        }
    }
}

fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

fn luma(c: Color) -> f32 {
    let (r, g, b) = Theme::rgb_of(c);
    (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) / 255.0
}

/// Blend from `a` to `b` by `k` in 0..=1.
fn mix(a: Color, b: Color, k: f32) -> Color {
    let (ar, ag, ab) = Theme::rgb_of(a);
    let (br, bg, bb) = Theme::rgb_of(b);
    let ch = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * k).round() as u8;
    Color::Rgb(ch(ar, br), ch(ag, bg), ch(ab, bb))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wordmark_rows_line_up() {
        let art = art();
        assert!(art.iter().all(|r| r.len() == art[0].len()));
    }

    #[test]
    fn mix_hits_both_ends() {
        let a = Color::Rgb(0, 100, 200);
        let b = Color::Rgb(255, 0, 50);
        assert_eq!(mix(a, b, 0.0), a);
        assert_eq!(mix(a, b, 1.0), b);
    }
}
