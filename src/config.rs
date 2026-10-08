use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

pub fn config_dir() -> PathBuf {
    std::env::var_os("MAXIMUS_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config/maximus"))
}

pub fn state_dir() -> PathBuf {
    std::env::var_os("MAXIMUS_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local/state/maximus"))
}

/// Expand a leading `~` to the home directory.
pub fn expand(p: &str) -> PathBuf {
    if p == "~" {
        home()
    } else if let Some(rest) = p.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(p)
    }
}

/// Shorten a path for display by replacing the home directory with `~`.
pub fn tilde(p: &Path) -> String {
    let h = home();
    match p.strip_prefix(&h) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub default_model: String,
    /// Empty string means "let claude decide".
    pub default_effort: String,
    /// Permission mode passed to claude --permission-mode; empty lets claude decide.
    pub default_mode: String,
    pub models: Vec<String>,
    pub project_roots: Vec<String>,
    pub worktree_dir: String,
    pub claude_command: String,
    pub theme: String,
    pub timeouts: Timeouts,
    pub notifications: Notifications,
    pub keys: Keys,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_model: "opus".into(),
            default_effort: String::new(),
            default_mode: String::new(),
            models: vec![
                "fable".into(),
                "opus".into(),
                "sonnet".into(),
                "haiku".into(),
            ],
            project_roots: vec!["~/Dev".into()],
            worktree_dir: "~/.local/share/maximus/worktrees".into(),
            claude_command: "claude".into(),
            theme: crate::theme::DEFAULT_ID.into(),
            timeouts: Timeouts::default(),
            notifications: Notifications::default(),
            keys: Keys::default(),
        }
    }
}

/// All values are minutes; 0 disables the timeout.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Timeouts {
    pub idle_kill: u32,
    pub archive_finished: u32,
    pub needs_input_nag: u32,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            idle_kill: 0,
            archive_finished: 60,
            needs_input_nag: 10,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Notifications {
    pub macos: bool,
    pub bell: bool,
    pub badge: bool,
}

impl Default for Notifications {
    fn default() -> Self {
        Self {
            macos: true,
            bell: true,
            badge: true,
        }
    }
}

/// Global key bindings. Each value may hold several space-separated keys.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Keys {
    pub switcher: String,
    pub new_prompt: String,
    pub diff: String,
    pub settings: String,
    pub next_session: String,
    pub prev_session: String,
    pub focus_list: String,
    pub focus_pane: String,
    pub graph: String,
    pub commit: String,
    pub terminal: String,
}

impl Default for Keys {
    fn default() -> Self {
        Self {
            switcher: "ctrl+p".into(),
            new_prompt: "ctrl+j".into(),
            diff: "ctrl+g".into(),
            settings: "ctrl+s".into(),
            next_session: "alt+down".into(),
            prev_session: "alt+up".into(),
            focus_list: "alt+left".into(),
            focus_pane: "alt+right".into(),
            graph: "ctrl+q".into(),
            commit: "ctrl+k".into(),
            terminal: "ctrl+t".into(),
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        config_dir().join("config.toml")
    }

    pub fn load() -> Self {
        let path = Self::path();
        match fs::read_to_string(&path) {
            Ok(s) => toml::from_str(&s).unwrap_or_else(|e| {
                eprintln!("maximus: ignoring invalid {}: {e}", path.display());
                Self::default()
            }),
            Err(_) => {
                let cfg = Self::default();
                let _ = cfg.save();
                cfg
            }
        }
    }

    pub fn save(&self) -> Result<()> {
        fs::create_dir_all(config_dir())?;
        let s = toml::to_string_pretty(self)?;
        fs::write(Self::path(), s).context("writing config")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectEntry {
    pub path: PathBuf,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: String,
    pub project: PathBuf,
    pub cwd: PathBuf,
    pub branch: Option<String>,
    pub name: String,
    pub model: String,
    pub effort: String,
    /// Unix seconds.
    pub created: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub projects: Vec<ProjectEntry>,
    pub list_width: u16,
    pub diff_tree_width: u16,
    pub diff_split: bool,
    pub graph_open: bool,
    /// Graph panel height in rows; 0 sizes it automatically.
    pub graph_height: u16,
    pub sessions: Vec<SessionRecord>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            projects: vec![],
            list_width: 32,
            diff_tree_width: 36,
            diff_split: true,
            graph_open: true,
            graph_height: 0,
            sessions: vec![],
        }
    }
}

impl State {
    fn path() -> PathBuf {
        state_dir().join("state.json")
    }

    pub fn load() -> Self {
        fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        fs::create_dir_all(state_dir())?;
        fs::write(Self::path(), serde_json::to_string_pretty(self)?).context("writing state")
    }
}
