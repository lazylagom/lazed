use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
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

pub fn socket_path() -> Result<PathBuf, String> {
    let status = run_cli(&["status", "--json"])?;
    status
        .pointer("/server/socket")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| "herdr status did not report a socket path".to_string())
}

pub fn snapshot() -> Result<Value, String> {
    let v = run_cli(&["api", "snapshot"])?;
    Ok(v
        .pointer("/result/snapshot")
        .cloned()
        .unwrap_or_else(|| v.get("result").cloned().unwrap_or(v)))
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

pub fn resize_pane(pane_id: &str, direction: &str, amount: f64) -> Result<Value, String> {
    run_cli(&[
        "pane",
        "resize",
        "--pane",
        pane_id,
        "--direction",
        direction,
        "--amount",
        &format!("{amount:.4}"),
    ])
}

pub fn workspace_create(cwd: Option<&str>, label: Option<&str>) -> Result<Value, String> {
    let mut args = vec!["workspace", "create"];
    if let Some(c) = cwd {
        args.extend(["--cwd", c]);
    }
    if let Some(l) = label {
        args.extend(["--label", l]);
    }
    run_cli(&args)
}

pub fn workspace_focus(workspace_id: &str) -> Result<Value, String> {
    run_cli(&["workspace", "focus", workspace_id])
}

pub fn workspace_close(workspace_id: &str) -> Result<Value, String> {
    run_cli(&["workspace", "close", workspace_id])
}

pub fn tab_create(workspace_id: &str, cwd: Option<&str>) -> Result<Value, String> {
    let mut args = vec!["tab", "create", "--workspace", workspace_id];
    if let Some(c) = cwd {
        args.extend(["--cwd", c]);
    }
    run_cli(&args)
}

pub fn tab_focus(tab_id: &str) -> Result<Value, String> {
    run_cli(&["tab", "focus", tab_id])
}

pub fn tab_close(tab_id: &str) -> Result<Value, String> {
    run_cli(&["tab", "close", tab_id])
}

pub fn agent_start(pane_id: &str, kind: &str, name: Option<&str>) -> Result<Value, String> {
    let args = ["agent", "start", name.unwrap_or(kind), "--kind", kind, "--pane", pane_id];
    run_cli(&args)
}

pub fn agent_prompt(pane_id: &str, text: &str) -> Result<Value, String> {
    run_cli(&["agent", "prompt", pane_id, text])
}

pub fn pane_send_text(pane_id: &str, text: &str) -> Result<Value, String> {
    run_cli(&["pane", "send-text", pane_id, text])
}

pub fn worktree_create(
    cwd: &str,
    branch: Option<&str>,
    label: Option<&str>,
    workspace: Option<&str>,
) -> Result<Value, String> {
    let mut args = vec!["worktree", "create", "--cwd", cwd];
    if let Some(b) = branch {
        args.extend(["--branch", b]);
    }
    if let Some(l) = label {
        args.extend(["--label", l]);
    }
    if let Some(w) = workspace {
        args.extend(["--workspace", w]);
    }
    run_cli(&args)
}

pub fn worktree_list() -> Result<Value, String> {
    run_cli(&["worktree", "list"])
}

/// Subscription types that apply globally (no pane_id required).
const GLOBAL_SUBS: &[&str] = &[
    "workspace.created",
    "workspace.updated",
    "workspace.renamed",
    "workspace.closed",
    "workspace.focused",
    "tab.created",
    "tab.closed",
    "tab.focused",
    "tab.renamed",
    "tab.moved",
    "pane.created",
    "pane.closed",
    "pane.updated",
    "pane.focused",
    "pane.exited",
    "pane.agent_detected",
    "layout.updated",
];

fn subscribe_request(subs: &Value) -> Value {
    json!({"id": "staylazy", "method": "events.subscribe", "params": {"subscriptions": subs}})
}

/// Connect the event socket, subscribe to global + per-pane events for `pane_ids`,
/// and call `on_line` for every subsequent JSON line until EOF/error.
/// Returns the error that ended the connection.
pub fn run_event_stream<F>(pane_ids: &[String], mut on_line: F) -> String
where
    F: FnMut(&Value, &mut dyn FnMut(&Value)),
{
    let result = (|| -> Result<(), String> {
        let path = socket_path()?;
        let sock = UnixStream::connect(&path)
            .map_err(|e| format!("failed to connect {}: {e}", path.display()))?;
        let mut writer = sock
            .try_clone()
            .map_err(|e| format!("failed to clone event socket: {e}"))?;
        let mut reader = BufReader::new(sock);

        let mut subs: Vec<Value> = GLOBAL_SUBS.iter().map(|t| json!({"type": t})).collect();
        for id in pane_ids {
            subs.push(json!({"type": "pane.agent_status_changed", "pane_id": id}));
        }
        let req = serde_json::to_string(&subscribe_request(&json!(subs))).map_err(|e| e.to_string())?;
        writer
            .write_all(req.as_bytes())
            .and_then(|_| writer.write_all(b"\n"))
            .and_then(|_| writer.flush())
            .map_err(|e| format!("failed to write subscription: {e}"))?;

        let mut add_sub = |v: &Value| {
            let line = serde_json::to_string(v).unwrap_or_default() + "\n";
            let _ = writer.write_all(line.as_bytes());
            let _ = writer.flush();
        };

        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => return Err("event socket closed".to_string()),
                Err(e) => return Err(format!("event socket read failed: {e}")),
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
                        on_line(&v, &mut add_sub);
                    }
                }
            }
        }
    })();
    result.err().unwrap_or_else(|| "event stream ended".to_string())
}

pub fn status_sub_for_pane(pane_id: &str) -> Value {
    subscribe_request(&json!([{"type": "pane.agent_status_changed", "pane_id": pane_id}]))
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
