use std::{
    io::Write,
    process::{Command, Stdio},
};

use base64::Engine;

fn osa_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Fire-and-forget macOS notification. Terminals that post notifications from an escape
/// sequence get one, so clicking it brings that terminal (and tab) forward; elsewhere osascript
/// posts it, which macOS attributes to Script Editor.
pub fn macos(title: &str, body: &str) {
    if !cfg!(target_os = "macos") {
        return;
    }
    if let Some(seq) = terminal_notification(term_program().as_deref(), title, body) {
        let mut out = std::io::stdout();
        let _ = out.write_all(seq.as_bytes());
        let _ = out.flush();
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

fn term_program() -> Option<String> {
    std::env::var("TERM_PROGRAM").ok()
}

/// Drop control characters so text can't end the escape sequence early.
fn osc_text(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

/// The escape sequence that makes `term` post a notification, if it supports one: iTerm2's
/// OSC 9 (body only), or OSC 777 (title and body) for Ghostty and WezTerm.
fn terminal_notification(term: Option<&str>, title: &str, body: &str) -> Option<String> {
    match term? {
        "iTerm.app" => Some(format!(
            "\x1b]9;{}: {}\x07",
            osc_text(title),
            osc_text(body)
        )),
        "ghostty" | "WezTerm" => Some(format!(
            "\x1b]777;notify;{};{}\x07",
            osc_text(title).replace(';', ","),
            osc_text(body)
        )),
        _ => None,
    }
}

pub fn bell() {
    let mut out = std::io::stdout();
    let _ = out.write_all(b"\x07");
    let _ = out.flush();
}

fn is_iterm() -> bool {
    term_program().is_some_and(|v| v == "iTerm.app")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_notification_per_terminal() {
        assert_eq!(
            terminal_notification(Some("iTerm.app"), "maximus · app", "Finished: fix"),
            Some("\x1b]9;maximus · app: Finished: fix\x07".to_string())
        );
        assert_eq!(
            terminal_notification(Some("ghostty"), "a;b", "c;d"),
            Some("\x1b]777;notify;a,b;c;d\x07".to_string())
        );
        assert_eq!(
            terminal_notification(Some("Apple_Terminal"), "t", "b"),
            None
        );
        assert_eq!(terminal_notification(Some("tmux"), "t", "b"), None);
        assert_eq!(terminal_notification(None, "t", "b"), None);
    }

    #[test]
    fn terminal_notification_strips_control_chars() {
        assert_eq!(
            terminal_notification(Some("iTerm.app"), "t\x07", "b\x1b]x"),
            Some("\x1b]9;t: b]x\x07".to_string())
        );
    }
}
