mod app;
mod config;
mod diff;
mod git;
mod hooks;
mod keys;
mod memory;
mod notify;
mod prompt;
mod session;
mod settings;
mod switcher;
mod syntax;
mod terminal;
mod textinput;
mod theme;
mod ui;

use std::{
    io::stdout,
    time::{Duration, Instant},
};

use anyhow::Result;
use crossbeam_channel::{RecvTimeoutError, unbounded};
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
        EnableFocusChange, EnableMouseCapture, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use app::{App, AppEvent};

const FRAME: Duration = Duration::from_millis(16);
const TICK: Duration = Duration::from_millis(100);

fn restore_terminal(kbd: bool) {
    let mut out = stdout();
    if kbd {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(
        out,
        DisableMouseCapture,
        DisableBracketedPaste,
        DisableFocusChange,
        LeaveAlternateScreen
    );
    let _ = disable_raw_mode();
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("hook") => {
            hooks::run_client();
            return Ok(());
        }
        Some("-h") | Some("--help") => {
            println!("maximus — a keyboard-first TUI for running Claude Code across projects\n");
            println!(
                "usage: maximus [folder]   open maximus (optionally adding a folder as a project)"
            );
            println!("config: {}", config::Config::path().display());
            return Ok(());
        }
        _ => {}
    }

    let (tx, rx) = unbounded::<AppEvent>();
    let hook_settings = hooks::write_settings_file()?;
    let sock = hooks::socket_path();
    hooks::start_server(&sock, tx.clone())?;
    diff::preload();

    let mut app = App::new(tx.clone(), hook_settings, sock.clone());
    // Add a folder passed on the command line, or the current repo on first sight.
    if let Some(p) = args.get(1) {
        let p = config::expand(p);
        if p.is_dir() {
            app.add_project(p);
        }
    } else if let Ok(cwd) = std::env::current_dir() {
        let known = app.state.projects.iter().any(|p| p.path == cwd);
        if !known
            && cwd != config::home()
            && git::is_repo(&cwd)
            && let Some(top) = git::toplevel(&cwd)
            && !app.state.projects.iter().any(|p| p.path == top)
        {
            app.add_project(top);
        }
    }

    enable_raw_mode()?;
    let mut out = stdout();
    execute!(
        out,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste,
        EnableFocusChange
    )?;
    let kbd = matches!(
        crossterm::terminal::supports_keyboard_enhancement(),
        Ok(true)
    );
    if kbd {
        execute!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
    }
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal(kbd);
        default_hook(info);
    }));

    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    terminal.clear()?;

    let input_tx = tx.clone();
    std::thread::spawn(move || {
        while let Ok(ev) = event::read() {
            if input_tx.send(AppEvent::Input(ev)).is_err() {
                break;
            }
        }
    });

    let result = run(&mut terminal, &mut app, rx);
    app.shutdown();
    restore_terminal(kbd);
    let _ = std::fs::remove_file(&sock);
    result
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
    rx: crossbeam_channel::Receiver<AppEvent>,
) -> Result<()> {
    let mut dirty = true;
    let mut last_draw = Instant::now() - FRAME;
    let mut last_tick = Instant::now();
    loop {
        if dirty && last_draw.elapsed() >= FRAME {
            terminal.draw(|f| ui::draw(f, app))?;
            last_draw = Instant::now();
            dirty = false;
        }
        if app.quit {
            return Ok(());
        }
        let until_tick = TICK.saturating_sub(last_tick.elapsed());
        let wait = if dirty {
            FRAME.saturating_sub(last_draw.elapsed()).min(until_tick)
        } else {
            until_tick
        };
        match rx.recv_timeout(wait) {
            Ok(ev) => {
                app.handle(ev);
                while let Ok(ev) = rx.try_recv() {
                    app.handle(ev);
                }
                dirty = true;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
        if last_tick.elapsed() >= TICK {
            last_tick = Instant::now();
            dirty |= app.on_tick();
        }
    }
}
