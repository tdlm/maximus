//! Claude Code hooks → maximus status events.
//!
//! Each `claude` we spawn gets `--settings <file>` registering `maximus hook` for the lifecycle
//! events we care about, plus `MAXIMUS_SESSION`/`MAXIMUS_SOCK` in its environment. The hook
//! subcommand forwards a compact JSON line over a unix socket to the running TUI.

use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    thread,
};

use anyhow::Result;
use crossbeam_channel::Sender;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{app::AppEvent, config::state_dir};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookMsg {
    pub sid: String,
    pub event: String,
    #[serde(default)]
    pub ntype: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub tool: String,
    #[serde(default)]
    pub prompt: String,
}

/// Unix socket paths are capped at ~104 bytes on macOS, so keep this short.
pub fn socket_path() -> PathBuf {
    let tmp = std::env::temp_dir();
    let p = tmp.join(format!("maximus-{}.sock", std::process::id()));
    if p.as_os_str().len() < 100 {
        p
    } else {
        PathBuf::from(format!("/tmp/maximus-{}.sock", std::process::id()))
    }
}

/// Write the settings file passed to every spawned claude via `--settings`.
pub fn write_settings_file() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let cmd = format!(
        "'{}' hook",
        exe.display().to_string().replace('\'', r"'\''")
    );
    let entry = |matcher: Option<&str>| {
        let mut v = json!({ "hooks": [{ "type": "command", "command": cmd, "timeout": 5 }] });
        if let Some(m) = matcher {
            v["matcher"] = json!(m);
        }
        json!([v])
    };
    let settings = json!({
        "hooks": {
            "UserPromptSubmit": entry(None),
            "PreToolUse": entry(Some("*")),
            "PostToolUse": entry(Some("*")),
            "Notification": entry(None),
            "Stop": entry(None),
        }
    });
    std::fs::create_dir_all(state_dir())?;
    let path = state_dir().join("hook-settings.json");
    std::fs::write(&path, serde_json::to_string_pretty(&settings)?)?;
    Ok(path)
}

/// `maximus hook`: runs inside claude's hook machinery. Must be quiet and always succeed.
pub fn run_client() {
    let (Ok(sid), Ok(sock)) = (
        std::env::var("MAXIMUS_SESSION"),
        std::env::var("MAXIMUS_SOCK"),
    ) else {
        return;
    };
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let v: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let mut prompt = s("prompt");
    if prompt.len() > 300 {
        let mut cut = 300;
        while !prompt.is_char_boundary(cut) {
            cut -= 1;
        }
        prompt.truncate(cut);
    }
    let msg = HookMsg {
        sid,
        event: s("hook_event_name"),
        ntype: s("notification_type"),
        message: s("message"),
        tool: s("tool_name"),
        prompt,
    };
    if let Ok(mut stream) = UnixStream::connect(&sock)
        && let Ok(line) = serde_json::to_string(&msg)
    {
        let _ = writeln!(stream, "{line}");
    }
}

/// Listen for hook messages and forward them to the app.
pub fn start_server(path: &Path, tx: Sender<AppEvent>) -> Result<()> {
    let _ = std::fs::remove_file(path);
    std::fs::create_dir_all(path.parent().unwrap())?;
    let listener = UnixListener::bind(path)?;
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let tx = tx.clone();
            thread::spawn(move || {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    if let Ok(msg) = serde_json::from_str::<HookMsg>(&line) {
                        let _ = tx.send(AppEvent::Hook(msg));
                    }
                }
            });
        }
    });
    Ok(())
}
