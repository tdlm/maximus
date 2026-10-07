use std::{
    io::Write,
    process::{Command, Stdio},
};

use base64::Engine;

fn osa_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Fire-and-forget macOS notification.
pub fn macos(title: &str, body: &str) {
    if !cfg!(target_os = "macos") {
        return;
    }
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        osa_escape(body),
        osa_escape(title)
    );
    let _ = Command::new("osascript")
        .args(["-e", &script])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

pub fn bell() {
    let mut out = std::io::stdout();
    let _ = out.write_all(b"\x07");
    let _ = out.flush();
}

fn is_iterm() -> bool {
    std::env::var("TERM_PROGRAM").is_ok_and(|v| v == "iTerm.app")
}

/// Set (or clear, with "") the iTerm2 badge. No-op in other terminals.
pub fn badge(text: &str) {
    if !is_iterm() {
        return;
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(text);
    let mut out = std::io::stdout();
    let _ = write!(out, "\x1b]1337;SetBadgeFormat={b64}\x07");
    let _ = out.flush();
}

pub fn copy(text: &str) {
    if cfg!(target_os = "macos")
        && let Ok(mut child) = Command::new("pbcopy").stdin(Stdio::piped()).spawn()
    {
        if let Some(stdin) = child.stdin.as_mut() {
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = child.wait();
        return;
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(text);
    let mut out = std::io::stdout();
    let _ = write!(out, "\x1b]52;c;{b64}\x07");
    let _ = out.flush();
}
