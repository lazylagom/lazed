//! lazed daemon client — unix socket NDJSON for API calls and the event
//! stream. The daemon owns the organization model (group > project >
//! herdr workspace annotations) and proxies herdr control through
//! `herdr.call`; terminal bytes go straight to herdr (see herdr.rs).
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::value::RawValue;
use serde_json::{json, Value};

use crate::env;

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
    // Development must launch the binary built by this checkout's make dev,
    // even if the installed CLI points at another checkout.
    if cfg!(debug_assertions) {
        let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../daemon/target/release/lazed");
        if dev.is_file() {
            return Ok(dev.to_string_lossy().into_owned());
        }
    }
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

/// Why a call failed: a transport break means the daemon may have been
/// replaced (possibly by an older build) — cached capability answers are
/// dropped; a remote error is the daemon's own reply and says nothing
/// about which build answered.
enum CallError {
    Transport(String),
    Remote(String),
}

/// `herdr.overlay.v1`, checked once per daemon lifetime. Cleared whenever
/// a gated call hits a transport failure or an "unknown method" reply —
/// either can mean the daemon was restarted to an older build.
static OVERLAY_CAPABLE: AtomicBool = AtomicBool::new(false);

const UPGRADE_REQUIRED: &str = "daemon_upgrade_required: the running lazed daemon predates the herdr overlay; restart it (herdr keeps the panes alive)";

fn overlay_capable() -> Result<bool, String> {
    let status = api_call("session.status", json!({}))?;
    let ok = status["capabilities"].as_array().is_some_and(|caps| caps.iter().any(|c| c == "herdr.overlay.v1"));
    if ok {
        OVERLAY_CAPABLE.store(true, Ordering::Relaxed);
    }
    Ok(ok)
}

/// One request/response call over the socket. `herdr.*`/`task.start` are
/// gated on the daemon's herdr.overlay.v1 capability so an old daemon
/// fails fast with an upgrade hint instead of a parse error; the check is
/// cached after first success and re-run whenever a call hints the daemon
/// process changed underneath us.
pub fn api_call(method: &str, params: Value) -> Result<Value, String> {
    let gated = method == "task.start" || method.starts_with("herdr.");
    if gated && !OVERLAY_CAPABLE.load(Ordering::Relaxed) && !overlay_capable()? {
        return Err(UPGRADE_REQUIRED.into());
    }
    match socket_call(method, params) {
        Ok(v) => Ok(v),
        Err(CallError::Transport(e)) => {
            if gated {
                OVERLAY_CAPABLE.store(false, Ordering::Relaxed);
            }
            Err(e)
        }
        Err(CallError::Remote(e)) => {
            // an older daemon answers "unknown method" where a new one has
            // the method — re-check so the caller still gets the designed
            // upgrade error rather than an opaque method name
            if gated && e.contains("unknown method") {
                OVERLAY_CAPABLE.store(false, Ordering::Relaxed);
                if matches!(overlay_capable(), Ok(false)) {
                    return Err(UPGRADE_REQUIRED.into());
                }
            }
            Err(e)
        }
    }
}

fn rpc_timeout(method: &str, params: &Value) -> Duration {
    if matches!(method, "session.status" | "herdr.status") { return Duration::from_secs(5); }
    if method == "task.start" { return Duration::from_secs(180); }
    if method == "herdr.call" {
        if params["method"] == "agent.start" { return Duration::from_secs(45); }
        if params["method"] == "agent.wait" {
            return Duration::from_millis(params["params"]["timeout_ms"].as_u64().unwrap_or(30_000).saturating_add(5_000));
        }
    }
    Duration::from_secs(30)
}

/// Socket round-trip: connect, write the request, read one reply line.
fn socket_call(method: &str, params: Value) -> Result<Value, CallError> {
    let transport = |e: String| CallError::Transport(e);
    let mut s = UnixStream::connect(sock_path())
        .map_err(|e| transport(format!("cannot connect {}: {e}", sock_path().display())))?;
    s.set_read_timeout(Some(rpc_timeout(method, &params))).map_err(|e| transport(e.to_string()))?;
    s.set_write_timeout(Some(Duration::from_secs(5))).map_err(|e| transport(e.to_string()))?;
    let req = json!({"id": 1, "method": method, "params": params});
    writeln!(s, "{}", serde_json::to_string(&req).map_err(|e| transport(e.to_string()))?)
        .map_err(|e| transport(e.to_string()))?;
    s.flush().map_err(|e| transport(e.to_string()))?;
    let mut line = String::new();
    let bytes = BufReader::new(s.try_clone().map_err(|e| transport(e.to_string()))?)
        .read_line(&mut line)
        .map_err(|e| transport(e.to_string()))?;
    if bytes == 0 {
        return Err(transport("daemon_disconnected".into()));
    }
    let v: Value = serde_json::from_str(line.trim()).map_err(|e| transport(e.to_string()))?;
    if let Some(e) = v.get("error") {
        return Err(CallError::Remote(e.as_str().unwrap_or("daemon error").to_string()));
    }
    Ok(v.get("result").cloned().unwrap_or(Value::Null))
}

fn server_running() -> bool {
    api_call("session.status", json!({}))
        .ok()
        .and_then(|v| v.get("running").and_then(Value::as_bool))
        .unwrap_or(false)
}

/// One herdr API call through the daemon's passthrough — tab/pane/agent
/// control stays on the single lazed socket the app already holds.
pub fn herdr_call(method: &str, params: Value) -> Result<Value, String> {
    api_call("herdr.call", json!({"method": method, "params": params}))
}

/// Ensure the daemon is up; spawn a detached `lazed server` if not.
/// The daemon outlives the app — on relaunch we just reconnect. It gets
/// the same herdr binary + session as the app so both see one herdr.
pub fn ensure_server() -> Result<(), String> {
    if server_running() {
        return Ok(());
    }
    let bin = lazed_bin()?;
    Command::new(bin)
        .arg("server")
        .envs(env::env_for_spawn())
        .envs(crate::herdr::env_for_daemon())
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

/// Stop the daemon and bring up a fresh one from the current binary.
/// The daemon respawns panes from its persisted session, but processes
/// that were running inside them are gone — callers confirm first.
pub fn restart_server(only_if_empty: bool) -> Result<Value, String> {
    let status = api_call("session.status", json!({}))?;
    let old_pid = status["pid"].clone();
    // This gate lives in the daemon, under its session lock. A client-side
    // count followed by server.stop could kill a newly created terminal.
    if only_if_empty {
        if !status["capabilities"].as_array().is_some_and(|caps|
            caps.iter().any(|c| c == "server.stop_if_empty.v1")) {
            return Err("daemon_manual_restart_required".into());
        }
    }
    let method = if only_if_empty { "server.stop_if_empty" } else { "server.stop" };
    match api_call(method, json!({})) {
        Ok(_) => {},
        // The atomic empty-session stop exits while still holding the lock.
        Err(e) if only_if_empty && e == "daemon_disconnected" => {},
        Err(e) => return Err(e),
    };
    // The event reconnect loop may have already started the replacement.
    // Wait for the old PID, not for every daemon to disappear.
    let old_running = || api_call("session.status", json!({}))
        .is_ok_and(|s| s["pid"] == old_pid);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && old_running() {
        std::thread::sleep(Duration::from_millis(100));
    }
    if old_running() {
        return Err("old lazed server did not exit".to_string());
    }
    ensure_server()?;
    api_call("session.status", json!({}))
}

/// The daemon's event stream — `{"event":…,"data":…}` lines forwarded as
/// raw JSON (no Value DOM round-trip per event; the webview parses once).
/// Returns why the stream ended; the caller decides whether to reconnect.
/// Closing/shutdown unsubscribes server-side.
pub fn run_event_stream(on_event: impl Fn(Box<RawValue>) -> bool) -> String {
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
                if let Ok(v) = serde_json::from_str::<Box<RawValue>>(trimmed) {
                    if !on_event(v) {
                        return "stopped".to_string();
                    }
                }
            }
        }
    }
}

/// `lazed doctor --json` — CLI link state for the onboarding banner.
/// doctor exits 1 when checks fail; the JSON is still valid, so parse
/// whatever it printed.
pub fn doctor_report() -> Result<Value, String> {
    let out = Command::new(lazed_bin()?)
        .args(["doctor", "--json"])
        .envs(env::env_for_spawn())
        .output()
        .map_err(|e| format!("lazed doctor: {e}"))?;
    serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("lazed doctor output: {e} ({:?})", out.stdout))
}

/// `lazed install` — non-interactive; a conflicting existing link needs a
/// terminal prompt, which fails here and surfaces as stderr text.
pub fn cli_install() -> Result<Value, String> {
    let out = Command::new(lazed_bin()?)
        .arg("install")
        .envs(env::env_for_spawn())
        .output()
        .map_err(|e| format!("lazed install: {e}"))?;
    if out.status.success() {
        Ok(json!({"ok": true, "output": String::from_utf8_lossy(&out.stdout)}))
    } else {
        Err(format!("lazed install failed: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// Probe installed agent CLIs — walk the resolved spawn PATH directly,
/// so GUI launches find Homebrew/version-manager installs too.
pub fn agent_detect() -> Value {
    // herdr 0.9.0 manifests lazed offers in the picker
    let kinds = [
        "claude", "codex", "devin", "gemini", "opencode", "pi", "cursor", "copilot", "amp", "kimi", "qwen",
    ];
    let spawn_env = env::env_for_spawn();
    let path = spawn_env.get("PATH").cloned().unwrap_or_default();
    let agents: Vec<Value> = kinds
        .iter()
        .map(|k| match env::find_on_path(k, &path) {
            Some(p) => json!({"kind": k, "path": p}),
            None => json!({"kind": k}),
        })
        .collect();
    json!({"context": "local", "agents": agents})
}
