//! Filesystem layout: ~/.local/state/lazed/{lazed.sock,session.json,lazed.log}
use std::path::PathBuf;

pub fn state_dir() -> PathBuf {
    if let Ok(d) = std::env::var("LAZED_STATE_DIR") {
        return PathBuf::from(d);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".local/state/lazed")
}

pub fn sock_path() -> PathBuf {
    state_dir().join("lazed.sock")
}

pub fn session_path() -> PathBuf {
    state_dir().join("session.json")
}

pub fn pid_path() -> PathBuf {
    state_dir().join("lazed.pid")
}

pub fn log_path() -> PathBuf {
    state_dir().join("lazed.log")
}

/// User config dir: ~/.config/lazed/ (override: LAZED_CONFIG_DIR).
pub fn config_dir() -> PathBuf {
    if let Ok(d) = std::env::var("LAZED_CONFIG_DIR") {
        return PathBuf::from(d);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".config/lazed")
}

/// Named agent launch specs — `agent.start` resolves `spec` through this.
pub fn agents_path() -> PathBuf {
    config_dir().join("agents.json")
}

/// Worktree checkouts: ~/.lazed/worktrees/<repo>/<branch-slug>
/// (override root: LAZED_WORKTREE_DIR).
pub fn worktrees_dir() -> PathBuf {
    if let Ok(d) = std::env::var("LAZED_WORKTREE_DIR") {
        return PathBuf::from(d);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".lazed/worktrees")
}

pub fn ensure_dir() -> std::io::Result<()> {
    std::fs::create_dir_all(state_dir())
}
