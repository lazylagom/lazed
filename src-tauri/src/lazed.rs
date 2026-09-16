//! lazed daemon client — unix socket NDJSON + `lazed term attach` control
//! streams. Replaces the herdr client: the daemon owns the model
//! (session > project > terminal), we just forward its protocol.
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

fn state_dir() -> PathBuf {
    if let Ok(d) = std::env::var("LAZED_STATE_DIR") {
        return PathBuf::from(d);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".local/state/lazed")
}

fn sock_path() -> PathBuf {
    state_dir().join("lazed.sock")
}

/// Resolved lazed binary: bundled resource first, then PATH.
static BUNDLED: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn register_bundled(path: Option<PathBuf>) {
    if let Ok(mut g) = BUNDLED.lock() {
        *g = path;
    }
}

fn lazed_bin() -> Result<String, String> {
    if let Ok(g) = BUNDLED.lock() {
        if let Some(p) = g.as_ref() {
            return Ok(p.to_string_lossy().to_string());
        }
    }
    // standard install location — Finder-launched apps don't inherit the
    // shell PATH, so check it explicitly before trusting PATH lookup
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home).join(".local/bin/lazed");
        if p.is_file() {
            return Ok(p.to_string_lossy().to_string());
        }
    }
    Ok("lazed".to_string())
}

/// One request/response call over the socket.
pub fn api_call(method: &str, params: Value) -> Result<Value, String> {
    let mut s = UnixStream::connect(sock_path())
        .map_err(|e| format!("cannot connect {}: {e}", sock_path().display()))?;
    let req = json!({"id": 1, "method": method, "params": params});
    writeln!(s, "{}", serde_json::to_string(&req).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    s.flush().map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(s.try_clone().map_err(|e| e.to_string())?)
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(line.trim()).map_err(|e| e.to_string())?;
    if let Some(e) = v.get("error") {
        return Err(e.as_str().unwrap_or("daemon error").to_string());
    }
    Ok(v.get("result").cloned().unwrap_or(Value::Null))
}

fn server_running() -> bool {
    api_call("session.status", json!({}))
        .ok()
        .and_then(|v| v.get("running").and_then(Value::as_bool))
        .unwrap_or(false)
}

/// Ensure the daemon is up; spawn a detached `lazed server` if not.
/// The daemon outlives the app — on relaunch we just reconnect.
pub fn ensure_server() -> Result<(), String> {
    if server_running() {
        return Ok(());
    }
    let bin = lazed_bin()?;
    Command::new(bin)
        .arg("server")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to spawn lazed server: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if server_running() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err("lazed server did not become ready within 10s".to_string())
}

/// An attached terminal's control stream (`lazed term attach` child).
pub struct ControlHandle {
    pub child: Child,
    pub stdin: ChildStdin,
}

impl ControlHandle {
    pub fn send(&mut self, msg: &Value) -> Result<(), String> {
        let mut line = serde_json::to_string(msg).map_err(|e| e.to_string())?;
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .and_then(|_| self.stdin.flush())
            .map_err(|e| e.to_string())
    }
}

/// Spawn `lazed term attach <id> --cols --rows` — stdout is the frame
/// stream, stdin takes {"type": ...} commands.
pub fn open_attach_stream(
    term_id: &str,
    cols: u32,
    rows: u32,
) -> Result<(Child, BufReader<std::process::ChildStdout>), String> {
    let mut child = Command::new(lazed_bin()?)
        .args([
            "term",
            "attach",
            term_id,
            "--cols",
            &cols.to_string(),
            "--rows",
            &rows.to_string(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to spawn lazed term attach: {e}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "attach: no stdout".to_string())?;
    Ok((child, BufReader::new(stdout)))
}

pub fn take_stdin(child: &mut Child) -> Result<ChildStdin, String> {
    child
        .stdin
        .take()
        .ok_or_else(|| "attach: no stdin".to_string())
}

// ── control-stream stdin commands ──────────────────────────────────

pub fn cmd_input_text(text: &str) -> Value {
    json!({"type": "input", "text": text})
}

pub fn cmd_resize(cols: u32, rows: u32) -> Value {
    json!({"type": "resize", "cols": cols, "rows": rows})
}

/// Pixel-granular scroll — the daemon accumulates sub-line remainders.
/// `delta_px` > 0 = scroll down (wheel convention).
pub fn cmd_scroll_px(
    delta_px: f64,
    cell_px: f64,
    column: u16,
    row: u16,
    modifiers: u8,
) -> Value {
    json!({
        "type": "scroll",
        "delta_px": delta_px,
        "cell_px": cell_px,
        "column": column,
        "row": row,
        "modifiers": modifiers,
    })
}

/// Line-granular scroll fallback (line/page delta modes).
pub fn cmd_scroll_lines(
    delta_lines: f64,
    column: u16,
    row: u16,
    modifiers: u8,
) -> Value {
    json!({
        "type": "scroll",
        "delta_lines": delta_lines,
        "column": column,
        "row": row,
        "modifiers": modifiers,
    })
}

/// Global event stream: opens a socket conn, sends `events.subscribe`,
/// and calls `on_event` for every pushed {"event","data"} line.
/// Returns when the connection drops (caller reconnects).
pub fn run_event_stream(on_event: impl Fn(&Value) -> bool) -> String {
    let conn = match UnixStream::connect(sock_path()) {
        Ok(c) => c,
        Err(e) => return format!("connect failed: {e}"),
    };
    let mut w = match conn.try_clone() {
        Ok(c) => c,
        Err(e) => return format!("clone failed: {e}"),
    };
    let sub = json!({"id": 1, "method": "events.subscribe", "params": {}});
    if writeln!(w, "{}", serde_json::to_string(&sub).unwrap_or_default()).is_err() {
        return "subscribe write failed".to_string();
    }
    let _ = w.flush();
    let mut line = String::new();
    let mut r = BufReader::new(conn);
    loop {
        line.clear();
        match r.read_line(&mut line) {
            Ok(0) => return "eof".to_string(),
            Err(e) => return format!("read failed: {e}"),
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
                    if !on_event(&v) {
                        return "stopped".to_string();
                    }
                }
            }
        }
    }
}

/// Probe installed agent CLIs — simple `which` per known kind.
pub fn agent_detect() -> Value {
    let kinds = [
        "claude", "codex", "antigravity", "devin", "gemini", "opencode", "aider", "pi",
    ];
    let agents: Vec<Value> = kinds
        .iter()
        .map(|k| {
            let path = Command::new("which")
                .arg(k)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|p| !p.is_empty());
            match path {
                Some(p) => json!({"kind": k, "path": p}),
                None => json!({"kind": k}),
            }
        })
        .collect();
    json!({"context": "local", "agents": agents})
}
