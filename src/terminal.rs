use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Instant,
};

use anyhow::Result;
use crossbeam_channel::Sender;
use portable_pty::CommandBuilder;

use crate::{
    app::AppEvent,
    session::{Pty, SCROLLBACK},
};

/// A login shell started in a checkout. It outlives the overlay showing it, so hiding the
/// terminal leaves whatever runs in it (a dev server, a watcher) going; it's gone once the
/// shell exits. A checkout can have several, shown as tabs.
pub struct Terminal {
    pub id: u64,
    pub cwd: PathBuf,
    pub parser: Arc<Mutex<vt100::Parser>>,
    pub pty: Pty,
    /// Lines scrolled back from the bottom (0 = live).
    pub scroll: usize,
    /// When the overlay last showed it, so ctrl+t brings back the last one used.
    pub shown: Instant,
}

impl Terminal {
    pub fn spawn(
        id: u64,
        cwd: PathBuf,
        size: (u16, u16),
        colors: ((u8, u8, u8), (u8, u8, u8)),
        tx: Sender<AppEvent>,
    ) -> Result<Self> {
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/bin/sh".into());
        let mut cmd = CommandBuilder::new(shell);
        // A login shell, as a new terminal window would start, so profiles set up PATH.
        cmd.arg("-l");
        cmd.cwd(&cwd);
        let parser = Arc::new(Mutex::new(vt100::Parser::new(
            size.0.max(5),
            size.1.max(20),
            SCROLLBACK,
        )));
        let pty = Pty::spawn(cmd, size, &parser, colors, tx, AppEvent::ShellExited(id))?;
        Ok(Self {
            id,
            cwd,
            parser,
            pty,
            scroll: 0,
            shown: Instant::now(),
        })
    }

    pub fn write(&mut self, bytes: &[u8]) {
        self.scroll = 0;
        self.pty.write(bytes);
    }

    /// Its tab's name: what's running in the foreground (`zsh`, `node`, `vim`, ...).
    pub fn label(&self) -> String {
        self.pty
            .foreground_pid()
            .and_then(process_name)
            .unwrap_or_else(|| "shell".into())
    }
}

#[cfg(target_os = "macos")]
fn process_name(pid: i32) -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: proc_name writes at most `buf.len()` bytes into `buf`.
    let n = unsafe { libc::proc_name(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    (n > 0).then(|| String::from_utf8_lossy(&buf[..n as usize]).into_owned())
}

#[cfg(not(target_os = "macos"))]
fn process_name(pid: i32) -> Option<String> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    Some(s.trim_end().to_string()).filter(|s| !s.is_empty())
}
