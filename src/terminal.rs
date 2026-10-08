use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
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
/// shell exits.
pub struct Terminal {
    pub id: u64,
    pub cwd: PathBuf,
    pub parser: Arc<Mutex<vt100::Parser>>,
    pub pty: Pty,
    /// Lines scrolled back from the bottom (0 = live).
    pub scroll: usize,
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
        })
    }

    pub fn write(&mut self, bytes: &[u8]) {
        self.scroll = 0;
        self.pty.write(bytes);
    }
}
