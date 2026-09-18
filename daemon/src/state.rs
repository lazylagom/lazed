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

/// Commit a complete private state file without truncating the previous copy.
pub fn atomic_write(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let parent = path.parent().ok_or_else(|| std::io::Error::other("missing parent"))?;
    std::fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(".state-{}.tmp", crate::control::id()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)?;
        std::fs::File::open(parent)?.sync_all()
    })();
    if result.is_err() { let _ = std::fs::remove_file(tmp); }
    result
}
