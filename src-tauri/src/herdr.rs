//! herdr execution-layer client for the app: server lifecycle and the
//! per-pane terminal stream. Everything else about herdr (tab/pane/agent
//! control, snapshots, events) reaches the app through the lazed daemon's
//! `herdr.call` passthrough and merged snapshot — this module is only the
//! byte path the daemon deliberately stays out of (PLAN §3b rule 1).
//!
//! Frames: `herdr terminal session control <pane> --takeover --cols --rows`
//! prints NDJSON `{"type":"terminal.frame","bytes":<base64 ANSI>,"full",
//! "seq","width","height"}` and takes `terminal.input|resize|scroll` JSON
//! lines on stdin.
use std::io::{BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::env;

/// herdr session the whole app (daemon + streams) attaches to. Named so a
/// bare `herdr` in the user's shell doesn't pick lazed's panes up by
/// accident; the daemon reads the same variable (`LAZED_HERDR_SESSION`).
pub fn session_name() -> String {
    std::env::var("LAZED_HERDR_SESSION").unwrap_or_else(|_| "lazed".into())
}

fn session_args() -> Vec<String> {
    let s = session_name();
    if s.is_empty() || s == "default" {
        vec![]
    } else {
        vec!["--session".into(), s]
    }
}

/// Bundled herdr (bundle.resources → Contents/Resources/bin/herdr).
static BUNDLED: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn register_bundled(path: Option<PathBuf>) {
    if let Ok(mut g) = BUNDLED.lock() {
        *g = path;
    }
}

/// Resolve the herdr binary: `HERDR_BIN` → bundled → PATH (login shell
/// PATH, so version-manager installs count) → known install dirs.
pub fn herdr_bin() -> Result<PathBuf, String> {
    if let Ok(p) = std::env::var("HERDR_BIN") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Ok(p);
        }
    }
    if let Ok(g) = BUNDLED.lock() {
        if let Some(p) = g.as_ref() {
            return Ok(p.clone());
        }
    }
    let path = env::env_for_spawn().get("PATH").cloned().unwrap_or_default();
    if let Some(p) = env::find_on_path("herdr", &path) {
        return Ok(PathBuf::from(p));
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    for c in [
        "/opt/homebrew/bin/herdr".to_string(),
        "/usr/local/bin/herdr".to_string(),
        format!("{home}/.local/bin/herdr"),
    ] {
        let p = PathBuf::from(&c);
        if p.is_file() {
            return Ok(p);
        }
    }
    Err("herdr_not_installed: herdr binary not found (bundle it, install it, or set HERDR_BIN)".into())
}

/// Environment the lazed daemon must share with the app so both talk to
/// the same herdr binary and session.
pub fn env_for_daemon() -> Vec<(String, String)> {
    let mut v = vec![("LAZED_HERDR_SESSION".to_string(), session_name())];
    if let Ok(p) = herdr_bin() {
        v.push(("HERDR_BIN".to_string(), p.to_string_lossy().to_string()));
    }
    v
}

fn herdr_cmd() -> Result<Command, String> {
    let mut c = Command::new(herdr_bin()?);
    c.args(session_args()).envs(env::env_for_spawn());
    Ok(c)
}

/// `herdr status --json` — works while the server is down.
pub fn status() -> Result<Value, String> {
    let out = herdr_cmd()?
        .args(["status", "--json"])
        .output()
        .map_err(|e| format!("herdr status: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .map(str::trim)
        .filter(|l| l.starts_with('{'))
        .find_map(|l| serde_json::from_str::<Value>(l).ok())
        .ok_or_else(|| {
            format!(
                "herdr status produced no JSON ({})",
                String::from_utf8_lossy(&out.stderr).trim()
            )
        })
}

pub fn server_running() -> bool {
    status()
        .ok()
        .and_then(|v| v.pointer("/server/running").and_then(Value::as_bool))
        .unwrap_or(false)
}

/// Ensure the herdr server for our session is up; spawn a detached
/// `herdr --session <s> server` if not. herdr outlives the app — panes and
/// agents keep running across relaunches.
pub fn ensure_server() -> Result<(), String> {
    if server_running() {
        return Ok(());
    }
    herdr_cmd()?
        .arg("server")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to spawn herdr server: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if server_running() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err("herdr server did not become ready within 10s".to_string())
}

// ── per-pane terminal stream ───────────────────────────────────────

pub struct ControlHandle {
    pub child: Child,
    stdin: ChildStdin,
}

impl ControlHandle {
    pub fn send(&mut self, cmd: &Value) -> Result<(), String> {
        let mut line = serde_json::to_string(cmd).map_err(|e| e.to_string())?;
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .and_then(|_| self.stdin.flush())
            .map_err(|e| e.to_string())
    }

    /// Stop the stream: closing stdin ends the control session; kill+reap
    /// covers a child that ignores EOF so no zombie is left behind.
    pub fn close(mut self) {
        drop(self.stdin);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Spawn `herdr terminal session control <pane> --takeover` and return the
/// handle for commands plus a reader of its NDJSON stdout.
pub fn open_control_stream(
    pane_id: &str,
    cols: u32,
    rows: u32,
) -> Result<(ControlHandle, BufReader<ChildStdout>), String> {
    let mut child = herdr_cmd()?
        .args([
            "terminal",
            "session",
            "control",
            pane_id,
            "--takeover",
            "--cols",
            &cols.to_string(),
            "--rows",
            &rows.to_string(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to spawn herdr control stream: {e}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "control stream missing stdout".to_string())?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "control stream missing stdin".to_string())?;
    Ok((ControlHandle { child, stdin }, BufReader::new(stdout)))
}

pub fn cmd_input_text(text: &str) -> Value {
    json!({"type": "terminal.input", "text": text})
}

pub fn cmd_resize(cols: u32, rows: u32) -> Value {
    json!({"type": "terminal.resize", "cols": cols, "rows": rows})
}

/// herdr scrolls its own scrollback by whole lines; `lines` > 0 scrolls
/// toward history, < 0 toward the live edge.
pub fn cmd_scroll(lines: i64) -> Option<Value> {
    if lines == 0 {
        return None;
    }
    Some(json!({
        "type": "terminal.scroll",
        "direction": if lines > 0 { "up" } else { "down" },
        "lines": lines.unsigned_abs(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_direction_and_magnitude() {
        assert_eq!(cmd_scroll(3).unwrap()["direction"], "up");
        assert_eq!(cmd_scroll(3).unwrap()["lines"], 3);
        assert_eq!(cmd_scroll(-2).unwrap()["direction"], "down");
        assert_eq!(cmd_scroll(-2).unwrap()["lines"], 2);
        assert!(cmd_scroll(0).is_none());
    }

    #[test]
    fn session_args_skip_default() {
        let a = session_args();
        assert!(a.is_empty() || a[0] == "--session");
    }
}
