use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// Active remote attachment: an SSH unix-socket forward to a remote herdr
/// server. When set, all CLI calls and streams route through `ssh <target>`.
pub struct Remote {
    pub target: String,
    /// Remote herdr session name; `None` = the remote default session.
    /// Mirrors `herdr --session <name>` on the remote host.
    pub session: Option<String>,
    pub local_socket: PathBuf,
    pub remote_socket: PathBuf,
    pub forward: Child,
}

fn spawn_forward(target: &str, local: &PathBuf, remote: &str) -> Result<Child, String> {
    let _ = std::fs::remove_file(local);
    Command::new("ssh")
        .args([
            "-N",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=8",
            "-o",
            "ExitOnForwardFailure=yes",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=2",
            "-L",
            &format!("{}:{}", local.display(), remote),
            "--",
            target,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to spawn ssh forward: {e}"))
}

static REMOTE: Mutex<Option<Remote>> = Mutex::new(None);

/// Write half of the live event-socket connection. Context switches
/// (remote attach/detach) shut it down so the event loop reconnects
/// against the new target.
static EVENT_WRITER: Mutex<Option<UnixStream>> = Mutex::new(None);

/// Bumped on every context switch. `run_event_stream` records it before
/// connecting and aborts the fresh connection if it changed — closes the
/// race where a kill lands between connect and writer registration.
static CONTEXT_GEN: AtomicU64 = AtomicU64::new(0);

pub fn kill_event_stream() {
    CONTEXT_GEN.fetch_add(1, Ordering::SeqCst);
    if let Ok(mut g) = EVENT_WRITER.lock() {
        if let Some(s) = g.take() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    }
}

pub fn remote_target() -> Option<String> {
    REMOTE.lock().ok()?.as_ref().map(|r| r.target.clone())
}

/// (target, session) of the active attachment — needed together when
/// building remote commands.
pub(crate) fn remote_ctx() -> Option<(String, Option<String>)> {
    REMOTE
        .lock()
        .ok()?
        .as_ref()
        .map(|r| (r.target.clone(), r.session.clone()))
}

/// A saved-machine session becomes ` --session '<name>'`; the default
/// session (or none) needs no flag — same as `herdr --session` semantics.
fn session_flag(session: Option<&str>) -> String {
    match session {
        Some(s) if !s.is_empty() && s != "default" => {
            format!(" --session {}", shell_quote(s))
        }
        _ => String::new(),
    }
}

pub(crate) fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Why an `ssh … herdr` call failed. `Transport` = ssh itself failed or the
/// remote produced no JSON at all (unreachable host, auth, missing remote
/// binary) — callers should bail fast instead of retrying. `Remote` = remote
/// herdr answered with an error object or unparseable JSON.
enum SshFail {
    Transport(String),
    Remote(String),
}

impl std::fmt::Display for SshFail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SshFail::Transport(m) | SshFail::Remote(m) => f.write_str(m),
        }
    }
}

fn ssh_run(target: &str, session: Option<&str>, args: &[&str]) -> Result<Value, SshFail> {
    let remote_cmd = format!("herdr{}", session_flag(session))
        + &args
            .iter()
            .map(|a| format!(" {}", shell_quote(a)))
            .collect::<String>();
    let output = Command::new("ssh")
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=8",
            "--",
            target,
            &remote_cmd,
        ])
        .output()
        .map_err(|e| SshFail::Transport(format!("failed to run ssh {target}: {e}")))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let line = stdout
        .lines()
        .find(|l| l.trim_start().starts_with('{'))
        .unwrap_or("")
        .to_string();
    if line.is_empty() {
        return Err(SshFail::Transport(format!(
            "ssh {target} herdr {:?} produced no JSON (status {:?}): {}",
            args,
            output.status.code(),
            stderr.trim()
        )));
    }
    let value: Value = serde_json::from_str(&line)
        .map_err(|e| SshFail::Remote(format!("bad JSON via ssh {:?}: {e}", args)))?;
    if let Some(err) = value.get("error") {
        return Err(SshFail::Remote(format!(
            "remote herdr {:?} error: {}",
            args, err
        )));
    }
    Ok(value)
}

/// Run a remote shell command over ssh (for non-herdr calls like `git` inside
/// remote worktrees). Returns the raw Output; the caller interprets status.
pub(crate) fn ssh_shell(target: &str, remote_cmd: &str) -> Result<std::process::Output, String> {
    Command::new("ssh")
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=8",
            "--",
            target,
            remote_cmd,
        ])
        .output()
        .map_err(|e| format!("failed to run ssh {target}: {e}"))
}

/// One detached `herdr server` start attempt on the remote host (`ssh -f`).
fn start_remote_server(target: &str, session: Option<&str>) {
    let cmd = format!("herdr{} server", session_flag(session));
    let _ = Command::new("ssh")
        .args([
            "-f",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=8",
            "--",
            target,
            &cmd,
        ])
        .status();
}

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

/// Bundled-binary override, registered once from `setup()` when the app
/// bundle ships herdr in resources (see scripts/fetch-herdr).
static BUNDLED_HERDR: OnceLock<Option<PathBuf>> = OnceLock::new();

pub fn register_bundled(path: Option<PathBuf>) {
    let _ = BUNDLED_HERDR.set(path);
}

pub fn herdr_bin() -> Result<PathBuf, String> {
    if let Some(Some(p)) = BUNDLED_HERDR.get() {
        return Ok(p.clone());
    }
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

/// Run `herdr <args>` (locally, or `ssh <target> herdr <args>` when a remote
/// machine is attached) and parse the single JSON response line.
pub fn run_cli(args: &[&str]) -> Result<Value, String> {
    if let Some((target, session)) = remote_ctx() {
        return ssh_run(&target, session.as_deref(), args).map_err(|e| e.to_string());
    }
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
/// When a remote machine is attached this instead verifies the SSH forward
/// is alive and the remote server responds — restarting it via ssh if down.
pub fn ensure_server() -> Result<(), String> {
    if let Some((target, session)) = remote_ctx() {
        ensure_remote_forward()?;
        match server_running() {
            Ok(true) => return Ok(()),
            // transport-level failure — the next reconnect-loop iteration
            // retries; a restart over a dead ssh link can't help anyway.
            Err(e) => return Err(e),
            // remote herdr answered but the server is down — try one
            // detached restart (mirrors the local auto-spawn below)
            Ok(false) => {
                start_remote_server(&target, session.as_deref());
                let deadline = Instant::now() + Duration::from_secs(10);
                while Instant::now() < deadline {
                    match server_running() {
                        Ok(true) => return Ok(()),
                        Ok(false) => std::thread::sleep(Duration::from_millis(500)),
                        Err(e) => return Err(e),
                    }
                }
                return Err("remote herdr server did not come up".to_string());
            }
        }
    }
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

/// `herdr status --json` — raw client/server/update status payload.
pub fn status() -> Result<Value, String> {
    run_cli(&["status", "--json"])
}

fn server_running() -> Result<bool, String> {
    Ok(status()?
        .pointer("/server/running")
        .and_then(Value::as_bool)
        .unwrap_or(false))
}

/// herdr version this build was tested against. Version drift isn't fatal —
/// pre-1.0 schema moves are the real risk — so it surfaces as a UI warning.
pub const EXPECTED_HERDR_VERSION: &str = "0.9.0";

/// Compatibility check against the live herdr server. Flags only explicit
/// incompatibilities and version drift; absent fields stay silent so a
/// schema change degrades to "unknown", not a false alarm.
pub fn compat_warning() -> Option<String> {
    let st = status().ok()?;
    let mut issues = Vec::new();
    if st.pointer("/server/compatible").and_then(Value::as_bool) == Some(false) {
        issues.push("client/server protocol incompatible".to_string());
    }
    if st
        .pointer("/server/endpoint_compatible")
        .and_then(Value::as_bool)
        == Some(false)
    {
        issues.push("terminal endpoint protocol incompatible".to_string());
    }
    let server_ver = st
        .pointer("/server/version")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    if server_ver != EXPECTED_HERDR_VERSION {
        issues.push(format!(
            "herdr server {server_ver} ≠ pinned {EXPECTED_HERDR_VERSION}"
        ));
    }
    if issues.is_empty() {
        None
    } else {
        Some(issues.join("; "))
    }
}

pub fn socket_path() -> Result<PathBuf, String> {
    if let Ok(guard) = REMOTE.lock() {
        if let Some(r) = guard.as_ref() {
            return Ok(r.local_socket.clone());
        }
    }
    let status = run_cli(&["status", "--json"])?;
    status
        .pointer("/server/socket")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| "herdr status did not report a socket path".to_string())
}

/// Uniquifier for the local end of forwarded sockets — a live attachment may
/// still hold the path from its own connect attempt.
static SOCK_SEQ: AtomicU64 = AtomicU64::new(0);

/// Attach to a remote herdr server over SSH:
/// `ssh target 'herdr status --json'` → forward remote unix socket to a local
/// path via `ssh -N -L`. All subsequent CLI/socket/stream traffic routes remote.
/// `session` selects a named remote herdr session (`None`/"default" = default).
pub fn remote_connect(target: &str, session: Option<&str>) -> Result<(), String> {
    let session = session.filter(|s| !s.is_empty() && *s != "default");
    // Probe the new target BEFORE dropping the current attachment so a failed
    // connect leaves the existing context (local or previous remote) intact.
    // The remote server must already run (herdr machine add prepares it); if
    // not, try one detached start before giving up.
    let mut status = ssh_run(target, session, &["status", "--json"]);
    if let Err(SshFail::Transport(e)) = &status {
        return Err(format!("remote unreachable: {e}"));
    }
    if status.is_err() || !status_ok(&status) {
        start_remote_server(target, session);
        let deadline = Instant::now() + Duration::from_secs(12);
        while Instant::now() < deadline {
            status = ssh_run(target, session, &["status", "--json"]);
            if status_ok(&status) {
                break;
            }
            if let Err(SshFail::Transport(e)) = &status {
                return Err(format!("remote unreachable: {e}"));
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    let status = status.map_err(|e| format!("remote herdr unreachable on {target}: {e}"))?;
    let remote_sock = status
        .pointer("/server/socket")
        .and_then(Value::as_str)
        .ok_or_else(|| "remote herdr status reported no socket".to_string())?
        .to_string();

    let seq = SOCK_SEQ.fetch_add(1, Ordering::Relaxed);
    let local_sock = std::env::temp_dir().join(format!(
        "staylazy-herdr-{}-{}.sock",
        std::process::id(),
        seq
    ));
    let mut forward = spawn_forward(target, &local_sock, &remote_sock)?;

    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        if local_sock.exists() && UnixStream::connect(&local_sock).is_ok() {
            // swap: install the new attachment, then kill + reap the old one
            let old = REMOTE.lock().map_err(|e| e.to_string())?.replace(Remote {
                target: target.to_string(),
                session: session.map(str::to_string),
                local_socket: local_sock,
                remote_socket: PathBuf::from(remote_sock),
                forward,
            });
            if let Some(mut o) = old {
                let _ = o.forward.kill();
                let _ = o.forward.wait();
                let _ = std::fs::remove_file(&o.local_socket);
            }
            kill_event_stream();
            return Ok(());
        }
        // ExitOnForwardFailure — no point waiting out the deadline
        if matches!(forward.try_wait(), Ok(Some(_))) {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let _ = forward.kill();
    let _ = forward.wait();
    let _ = std::fs::remove_file(&local_sock);
    Err(format!("ssh forward to {target} did not come up"))
}

fn status_ok(status: &Result<Value, SshFail>) -> bool {
    status
        .as_ref()
        .ok()
        .and_then(|s| s.pointer("/server/running").and_then(Value::as_bool))
        .unwrap_or(false)
}

/// Respawn the SSH socket forward if the child died (reconnect path).
fn ensure_remote_forward() -> Result<(), String> {
    // respawn under the lock, but poll for readiness after releasing it so
    // remote_target()/remote_disconnect stay responsive during the wait
    let local_sock = {
        let mut guard = REMOTE.lock().map_err(|e| e.to_string())?;
        let r = guard
            .as_mut()
            .ok_or_else(|| "no remote attached".to_string())?;
        if matches!(r.forward.try_wait(), Ok(None)) {
            return Ok(());
        }
        r.forward = spawn_forward(
            &r.target,
            &r.local_socket,
            &r.remote_socket.to_string_lossy(),
        )?;
        r.local_socket.clone()
    };
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        if local_sock.exists() && UnixStream::connect(&local_sock).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err("ssh forward respawn did not come up".to_string())
}

pub fn remote_disconnect() -> Result<(), String> {
    if let Some(mut r) = REMOTE.lock().map_err(|e| e.to_string())?.take() {
        let _ = r.forward.kill();
        let _ = r.forward.wait();
        let _ = std::fs::remove_file(&r.local_socket);
    }
    kill_event_stream();
    Ok(())
}

/// `herdr <args>` on the LOCAL client. Saved machines are local client state,
/// so machine management never routes through an attached remote.
fn run_local(args: &[&str]) -> Result<std::process::Output, String> {
    Command::new(herdr_bin()?)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run herdr {:?}: {e}", args))
}

/// Mutations print human text rather than JSON — success means exit 0.
fn local_ok(out: std::process::Output, args: &[&str]) -> Result<(), String> {
    if out.status.success() {
        return Ok(());
    }
    let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(if msg.is_empty() {
        format!("herdr {:?} failed (status {:?})", args, out.status.code())
    } else {
        msg
    })
}

/// `herdr machine list --json`
pub fn machine_list() -> Result<Value, String> {
    let output = run_local(&["machine", "list", "--json"])?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(stdout.trim())
        .map_err(|e| format!("bad JSON from herdr machine list: {e}"))
}

pub fn machine_remove(id: &str) -> Result<(), String> {
    local_ok(run_local(&["machine", "remove", id])?, &["machine", "remove", id])
}

pub fn machine_rename(id: &str, label: &str) -> Result<(), String> {
    local_ok(
        run_local(&["machine", "rename", id, "--label", label])?,
        &["machine", "rename", id, "--label", label],
    )
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

pub fn ensure_workspace(cwd: Option<&str>) -> Result<Value, String> {
    let v = run_cli(&["workspace", "list"])?;
    let workspaces = v
        .pointer("/result/workspaces")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if let Some(ws) = workspaces.into_iter().next() {
        return Ok(ws);
    }
    let mut args = vec!["workspace", "create"];
    if let Some(c) = cwd {
        args.extend(["--cwd", c]);
    }
    args.extend(["--label", "main"]);
    let v = run_cli(&args)?;
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

pub fn workspace_rename(workspace_id: &str, label: &str) -> Result<Value, String> {
    run_cli(&["workspace", "rename", workspace_id, label])
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

pub fn tab_rename(tab_id: &str, label: &str) -> Result<Value, String> {
    run_cli(&["tab", "rename", tab_id, label])
}

pub fn agent_start(pane_id: &str, kind: &str, name: Option<&str>) -> Result<Value, String> {
    let args = ["agent", "start", name.unwrap_or(kind), "--kind", kind, "--pane", pane_id];
    run_cli(&args)
}

pub fn agent_prompt(pane_id: &str, text: &str) -> Result<Value, String> {
    run_cli(&["agent", "prompt", pane_id, text])
}

/// `herdr agent get <pane>` → the agent object (unwrapped from the envelope).
pub fn agent_get(pane_id: &str) -> Result<Value, String> {
    let v = run_cli(&["agent", "get", pane_id])?;
    Ok(v.pointer("/result/agent").cloned().unwrap_or(v))
}

pub fn pane_send_text(pane_id: &str, text: &str) -> Result<Value, String> {
    run_cli(&["pane", "send-text", pane_id, text])
}

pub fn worktree_create(
    cwd: &str,
    branch: Option<&str>,
    base: Option<&str>,
    label: Option<&str>,
    workspace: Option<&str>,
) -> Result<Value, String> {
    let mut args = vec!["worktree", "create", "--cwd", cwd];
    if let Some(b) = branch {
        args.extend(["--branch", b]);
    }
    if let Some(b) = base {
        args.extend(["--base", b]);
    }
    if let Some(l) = label {
        args.extend(["--label", l]);
    }
    if let Some(w) = workspace {
        args.extend(["--workspace", w]);
    }
    run_cli(&args)
}

pub fn worktree_list(cwd: Option<&str>) -> Result<Value, String> {
    let mut args = vec!["worktree", "list"];
    if let Some(c) = cwd {
        args.extend(["--cwd", c]);
    }
    run_cli(&args)
}

pub fn worktree_remove(workspace_id: &str, force: bool) -> Result<Value, String> {
    let mut args = vec!["worktree", "remove", "--workspace", workspace_id];
    if force {
        args.push("--force");
    }
    run_cli(&args)
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
        let gen0 = CONTEXT_GEN.load(Ordering::SeqCst);
        let path = socket_path()?;
        let sock = UnixStream::connect(&path)
            .map_err(|e| format!("failed to connect {}: {e}", path.display()))?;
        let mut writer = sock
            .try_clone()
            .map_err(|e| format!("failed to clone event socket: {e}"))?;
        if let Ok(mut g) = EVENT_WRITER.lock() {
            *g = sock.try_clone().ok();
        }
        if CONTEXT_GEN.load(Ordering::SeqCst) != gen0 {
            // context switched mid-connect — this stream is bound to the
            // old target; drop it and let the loop reconnect fresh
            kill_event_stream();
            return Err("context switched during event connect".to_string());
        }
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
    let mut cmd = if let Some((target, session)) = remote_ctx() {
        let mut c = Command::new("ssh");
        c.args([
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=8",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=2",
            "--",
            &target,
            &format!(
                "herdr{} terminal session control {} --takeover --cols {} --rows {}",
                session_flag(session.as_deref()),
                shell_quote(pane_id),
                cols,
                rows
            ),
        ]);
        c
    } else {
        let mut c = Command::new(herdr_bin()?);
        c.args([
            "terminal",
            "session",
            "control",
            pane_id,
            "--takeover",
            "--cols",
            &cols.to_string(),
            "--rows",
            &rows.to_string(),
        ]);
        c
    };
    let mut child = cmd
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_wraps_and_escapes() {
        assert_eq!(shell_quote("w1:p1"), "'w1:p1'");
        assert_eq!(shell_quote(""), "''");
        // embedded single quote → end-quote, escaped quote, re-open
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        // no way to break out of the single-quoted string
        assert_eq!(
            shell_quote("x'; rm -rf /; echo '"),
            r"'x'\''; rm -rf /; echo '\'''"
        );
        assert_eq!(shell_quote("a\nb"), "'a\nb'");
        assert_eq!(shell_quote("$(whoami)`id`"), "'$(whoami)`id`'");
    }

    #[test]
    fn session_flag_only_for_named_sessions() {
        assert_eq!(session_flag(None), "");
        assert_eq!(session_flag(Some("")), "");
        assert_eq!(session_flag(Some("default")), "");
        assert_eq!(session_flag(Some("work")), " --session 'work'");
        assert_eq!(session_flag(Some("a b")), " --session 'a b'");
    }

    #[test]
    fn remote_connect_unreachable_host_errors_clean() {
        // bogus host → ssh transport failure → Err, and nothing stays attached
        let res = remote_connect("staylazy-no-such-host.invalid", None);
        assert!(res.is_err());
        assert!(remote_target().is_none());
    }
}
