use std::io::{BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

#[derive(Debug)]
pub struct ControlHandle {
    pub child: Child,
    pub stdin: ChildStdin,
}

impl ControlHandle {
    pub fn send(&mut self, cmd: &Value) -> Result<(), String> {
        let line = serde_json::to_string(cmd).map_err(|e| e.to_string())?;
        self.stdin
            .write_all(line.as_bytes())
            .and_then(|_| self.stdin.write_all(b"\n"))
            .and_then(|_| self.stdin.flush())
            .map_err(|e| e.to_string())
    }
}

pub fn herdr_bin() -> Result<PathBuf, String> {
    if let Ok(custom) = std::env::var("HERDR_BIN") {
        let p = PathBuf::from(custom);
        if p.is_file() {
            return Ok(p);
        }
    }
    if let Some(path_env) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_env) {
            let candidate = dir.join("herdr");
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    for candidate in [
        home.join(".local/share/mise/shims/herdr"),
        PathBuf::from("/opt/homebrew/bin/herdr"),
        PathBuf::from("/usr/local/bin/herdr"),
        home.join(".local/bin/herdr"),
    ] {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err("herdr binary not found on PATH or known install locations".to_string())
}

/// Run `herdr <args>` and parse the single JSON response line.
pub fn run_cli(args: &[&str]) -> Result<Value, String> {
    let bin = herdr_bin()?;
    let output = Command::new(bin)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run herdr {:?}: {e}", args))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let line = stdout
        .lines()
        .find(|l| l.trim_start().starts_with('{'))
        .unwrap_or("")
        .to_string();
    if line.is_empty() {
        return Err(format!(
            "herdr {:?} produced no JSON output (status {:?}): {}",
            args,
            output.status.code(),
            stderr.trim()
        ));
    }
    let value: Value =
        serde_json::from_str(&line).map_err(|e| format!("bad JSON from herdr {:?}: {e}", args))?;
    if let Some(err) = value.get("error") {
        return Err(format!("herdr {:?} error: {}", args, err));
    }
    Ok(value)
}

/// Ensure a herdr server is running; spawn a detached `herdr server` if not.
pub fn ensure_server() -> Result<(), String> {
    if server_running()? {
        return Ok(());
    }
    let bin = herdr_bin()?;
    Command::new(bin)
        .arg("server")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to spawn herdr server: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if server_running()? {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err("herdr server did not become ready within 10s".to_string())
}

fn server_running() -> Result<bool, String> {
    let status = run_cli(&["status", "--json"])?;
    Ok(status
        .pointer("/server/running")
        .and_then(Value::as_bool)
        .unwrap_or(false))
}

pub fn list_panes() -> Result<Vec<Value>, String> {
    let v = run_cli(&["pane", "list"])?;
    Ok(v
        .pointer("/result/panes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

pub fn ensure_workspace(cwd: &str) -> Result<Value, String> {
    let v = run_cli(&["workspace", "list"])?;
    let workspaces = v
        .pointer("/result/workspaces")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if let Some(ws) = workspaces.into_iter().next() {
        return Ok(ws);
    }
    let v = run_cli(&["workspace", "create", "--cwd", cwd, "--label", "main"])?;
    Ok(v.pointer("/result/workspace").cloned().unwrap_or(v))
}

pub fn split_pane(pane_id: &str, direction: &str) -> Result<Value, String> {
    run_cli(&["pane", "split", pane_id, "--direction", direction])
}

pub fn run_in_pane(pane_id: &str, command: &str) -> Result<Value, String> {
    run_cli(&["pane", "run", pane_id, command])
}

pub fn close_pane(pane_id: &str) -> Result<Value, String> {
    run_cli(&["pane", "close", pane_id])
}

/// Spawn `herdr terminal session control <pane> --takeover` and return the child
/// plus a reader of its stdout. Callers consume NDJSON `terminal.frame` lines.
pub fn open_control_stream(
    pane_id: &str,
    cols: u32,
    rows: u32,
) -> Result<(Child, BufReader<std::process::ChildStdout>), String> {
    let bin = herdr_bin()?;
    let mut child = Command::new(bin)
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
    Ok((child, BufReader::new(stdout)))
}

pub fn take_stdin(child: &mut Child) -> Result<ChildStdin, String> {
    child
        .stdin
        .take()
        .ok_or_else(|| "control stream missing stdin".to_string())
}

pub fn cmd_input_text(text: &str) -> Value {
    json!({"type": "terminal.input", "text": text})
}

pub fn cmd_resize(cols: u32, rows: u32) -> Value {
    json!({"type": "terminal.resize", "cols": cols, "rows": rows})
}
