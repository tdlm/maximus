use std::{
    io::{Read, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use crossbeam_channel::Sender;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::{app::AppEvent, config::SessionRecord};

pub const SCROLLBACK: usize = 5000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Working,
    NeedsInput,
    Done,
    Idle,
    Stopped,
}

impl Status {
    pub fn is_live(self) -> bool {
        !matches!(self, Status::Stopped)
    }
}

pub struct Pty {
    master: Box<dyn MasterPty + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    pub size: (u16, u16),
}

pub struct Session {
    pub id: String,
    pub project: PathBuf,
    pub cwd: PathBuf,
    pub branch: Option<String>,
    pub name: String,
    pub model: String,
    pub effort: String,
    pub created: u64,
    pub status: Status,
    pub seen: bool,
    pub archived: bool,
    pub last_change: Instant,
    pub last_notified: Option<Instant>,
    pub parser: Arc<Mutex<vt100::Parser>>,
    pub pty: Option<Pty>,
    /// Lines scrolled back from the bottom (0 = live).
    pub scroll: usize,
    /// Bumped on each spawn so stale exit events from a previous process are ignored.
    pub generation: u64,
}

pub struct Launch<'a> {
    pub claude: &'a str,
    pub settings: &'a PathBuf,
    pub sock: &'a PathBuf,
    pub prompt: Option<&'a str>,
    pub resume: bool,
    pub size: (u16, u16),
    pub term_bg: (u8, u8, u8),
    pub term_fg: (u8, u8, u8),
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Session {
    pub fn new(
        project: PathBuf,
        cwd: PathBuf,
        branch: Option<String>,
        name: String,
        model: String,
        effort: String,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            project,
            cwd,
            branch,
            name,
            model,
            effort,
            created: now_secs(),
            status: Status::Stopped,
            seen: true,
            archived: false,
            last_change: Instant::now(),
            last_notified: None,
            parser: Arc::new(Mutex::new(vt100::Parser::new(24, 80, SCROLLBACK))),
            pty: None,
            scroll: 0,
            generation: 0,
        }
    }

    pub fn from_record(r: &SessionRecord) -> Self {
        let mut s = Self::new(
            r.project.clone(),
            r.cwd.clone(),
            r.branch.clone(),
            r.name.clone(),
            r.model.clone(),
            r.effort.clone(),
        );
        s.id = r.id.clone();
        s.created = r.created;
        s.archived = true;
        s
    }

    pub fn record(&self) -> SessionRecord {
        SessionRecord {
            id: self.id.clone(),
            project: self.project.clone(),
            cwd: self.cwd.clone(),
            branch: self.branch.clone(),
            name: self.name.clone(),
            model: self.model.clone(),
            effort: self.effort.clone(),
            created: self.created,
        }
    }

    pub fn set_status(&mut self, s: Status) {
        if self.status != s {
            self.status = s;
            self.last_change = Instant::now();
        }
    }

    pub fn spawn(&mut self, l: Launch, tx: Sender<AppEvent>) -> Result<()> {
        let (rows, cols) = (l.size.0.max(5), l.size.1.max(20));
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("opening pty")?;

        let mut cmd = CommandBuilder::new(l.claude);
        if l.resume {
            cmd.args(["--resume", &self.id]);
        } else {
            cmd.args(["--session-id", &self.id]);
        }
        if !self.model.is_empty() {
            cmd.args(["--model", &self.model]);
        }
        if !self.effort.is_empty() {
            cmd.args(["--effort", &self.effort]);
        }
        cmd.arg("--settings");
        cmd.arg(l.settings);
        if !self.name.is_empty() && !l.resume {
            cmd.args(["--name", &self.name]);
        }
        if let Some(p) = l.prompt.filter(|p| !p.trim().is_empty()) {
            cmd.arg(p);
        }
        cmd.cwd(&self.cwd);
        for k in [
            "TERM_PROGRAM",
            "TERM_PROGRAM_VERSION",
            "ITERM_SESSION_ID",
            "LC_TERMINAL",
            "LC_TERMINAL_VERSION",
        ] {
            cmd.env_remove(k);
        }
        // If maximus itself runs inside a Claude Code session, don't let the children think
        // they're that session's subprocesses. User config (API keys, Bedrock, etc.) passes through.
        for (k, _) in std::env::vars_os() {
            let k = k.to_string_lossy();
            let host_session = k == "CLAUDECODE"
                || k == "CLAUDE_PID"
                || k == "CLAUDE_EFFORT"
                || k == "AI_AGENT"
                || k.starts_with("CLAUDE_AGENT_SDK")
                || k.starts_with("CLAUDE_CODE_SESSION")
                || k.starts_with("CLAUDE_CODE_SDK")
                || k.starts_with("CLAUDE_CODE_MESSAGING")
                || k.starts_with("CLAUDE_CODE_HOST")
                || k.starts_with("CLAUDE_CODE_OAUTH")
                || matches!(
                    k.as_ref(),
                    "CLAUDE_CODE_ENTRYPOINT"
                        | "CLAUDE_CODE_CHILD_SESSION"
                        | "CLAUDE_CODE_EXECPATH"
                        | "CLAUDE_CODE_DESKTOP_APP_VERSION"
                        | "CLAUDE_CODE_TERMINAL_MCP_TOOLS"
                        | "CLAUDE_CODE_DISABLE_TERMINAL_TITLE"
                        | "CLAUDE_CODE_EAGER_FLUSH"
                        | "CLAUDE_CODE_REPORT_FINDINGS"
                        | "CLAUDE_CODE_EMIT_TOOL_USE_SUMMARIES"
                        | "CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING"
                        | "CLAUDE_CODE_ORGANIZATION_UUID"
                        | "CLAUDE_CODE_ACCOUNT_UUID"
                        | "CLAUDE_CODE_USER_EMAIL"
                );
            if host_session {
                cmd.env_remove(k.as_ref());
            }
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("MAXIMUS_SESSION", &self.id);
        cmd.env("MAXIMUS_SOCK", l.sock);

        let mut child = pair.slave.spawn_command(cmd).context("spawning claude")?;
        drop(pair.slave);
        let killer = child.clone_killer();
        let mut reader = pair.master.try_clone_reader()?;
        let writer: Arc<Mutex<Box<dyn Write + Send>>> =
            Arc::new(Mutex::new(pair.master.take_writer()?));

        *self.parser.lock().unwrap() = vt100::Parser::new(rows, cols, SCROLLBACK);
        self.scroll = 0;
        self.generation += 1;
        let generation = self.generation;

        let parser = self.parser.clone();
        let reply = writer.clone();
        let tx2 = tx.clone();
        let (bg, fg) = (l.term_bg, l.term_fg);
        thread::spawn(move || {
            let mut buf = [0u8; 16384];
            let mut q = QueryScanner::default();
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let chunk = &buf[..n];
                        let answers = {
                            let mut p = parser.lock().unwrap();
                            p.process(chunk);
                            q.scan(chunk, p.screen(), bg, fg)
                        };
                        if !answers.is_empty() {
                            let mut w = reply.lock().unwrap();
                            let _ = w.write_all(&answers);
                            let _ = w.flush();
                        }
                        let _ = tx2.send(AppEvent::Output);
                    }
                }
            }
        });
        let id = self.id.clone();
        thread::spawn(move || {
            let _ = child.wait();
            let _ = tx.send(AppEvent::Exited(id, generation));
        });

        self.pty = Some(Pty {
            master: pair.master,
            writer,
            killer,
            size: (rows, cols),
        });
        // Stays idle until claude's UserPromptSubmit hook confirms it picked up the prompt
        // (first-run/trust dialogs can block it).
        self.status = Status::Idle;
        self.last_change = Instant::now();
        self.seen = true;
        Ok(())
    }

    pub fn write(&self, bytes: &[u8]) {
        if let Some(pty) = &self.pty {
            let mut w = pty.writer.lock().unwrap();
            let _ = w.write_all(bytes);
            let _ = w.flush();
        }
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let (rows, cols) = (rows.max(5), cols.max(20));
        if let Some(pty) = &mut self.pty
            && pty.size != (rows, cols)
        {
            pty.size = (rows, cols);
            let _ = pty.master.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            });
            self.parser.lock().unwrap().set_size(rows, cols);
        }
    }

    pub fn kill(&mut self) {
        if let Some(mut pty) = self.pty.take() {
            let _ = pty.killer.kill();
        }
        self.set_status(Status::Stopped);
    }
}

/// Answers the handful of terminal queries TUI programs block on (DA1, DSR, OSC 10/11).
#[derive(Default)]
struct QueryScanner {
    tail: Vec<u8>,
}

impl QueryScanner {
    fn scan(
        &mut self,
        chunk: &[u8],
        screen: &vt100::Screen,
        bg: (u8, u8, u8),
        fg: (u8, u8, u8),
    ) -> Vec<u8> {
        let mut data = std::mem::take(&mut self.tail);
        data.extend_from_slice(chunk);
        let mut out = Vec::new();
        let mut i = 0;
        while i < data.len() {
            if data[i] != 0x1b {
                i += 1;
                continue;
            }
            let rest = &data[i..];
            const PATS: [&[u8]; 6] = [
                b"\x1b[c",
                b"\x1b[0c",
                b"\x1b[6n",
                b"\x1b[5n",
                b"\x1b]11;?",
                b"\x1b]10;?",
            ];
            if PATS
                .iter()
                .any(|p| rest.len() < p.len() && p.starts_with(rest))
            {
                // A query split across reads; finish it with the next chunk.
                self.tail = rest.to_vec();
                break;
            }
            if rest.starts_with(b"\x1b[c") || rest.starts_with(b"\x1b[0c") {
                out.extend_from_slice(b"\x1b[?62;22c");
            } else if rest.starts_with(b"\x1b[6n") {
                let (r, c) = screen.cursor_position();
                out.extend_from_slice(format!("\x1b[{};{}R", r + 1, c + 1).as_bytes());
            } else if rest.starts_with(b"\x1b[5n") {
                out.extend_from_slice(b"\x1b[0n");
            } else if rest.starts_with(b"\x1b]11;?") || rest.starts_with(b"\x1b]10;?") {
                let (n, (r, g, b)) = if rest[3] == b'1' { (11, bg) } else { (10, fg) };
                out.extend_from_slice(
                    format!("\x1b]{n};rgb:{r:02x}{r:02x}/{g:02x}{g:02x}/{b:02x}{b:02x}\x1b\\")
                        .as_bytes(),
                );
            }
            i += 1;
        }
        if self.tail.len() > 64 {
            self.tail.clear();
        }
        out
    }
}
