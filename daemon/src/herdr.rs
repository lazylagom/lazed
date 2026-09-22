//! herdr adapter — the only place the lazed daemon touches the herdr server.
//!
//! Path C (PLAN §3b): herdr owns the execution layer (PTY, frame stream,
//! agent detection, restore, remote); lazed owns the organization layer and
//! refers to herdr panes by id only. This module is that seam:
//!
//! - `call`      one request/response over herdr's unix socket (herdr serves
//!               one request per connection, so every call is a short-lived
//!               connection — same as the v2 `api_call`)
//! - `status`    `herdr status --json` — socket path, version, compat flags.
//!               Works while the server is down, so it is also the discovery.
//! - `spawn_bridge`
//!               a thread that subscribes to herdr's event stream and
//!               re-broadcasts every event on the lazed bus as
//!               `herdr.<event>`; reconnects with backoff and announces
//!               `herdr.connected` (carrying a fresh snapshot for reconcile)
//!               and `herdr.disconnected` so subscribers never trust stale
//!               pane state after a gap.
//!
//! The daemon never spawns `herdr server` (rule 4: the two daemons don't
//! spawn each other — the app auto-detect-launches both). Without a
//! running herdr every call here fails fast with `herdr_not_running` and
//! the rest of lazed keeps working (degraded mode).
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::session::Session;

/// Pinned herdr release. `compat_warning` flags a server that isn't this
/// version — pre-1.0 schema drift is an explicit upgrade event
/// (`scripts/check_herdr_schema.py`), never a silent runtime surprise.
pub const EXPECTED_VERSION: &str = "0.9.0";

/// herdr session lazed attaches to. Named so a stray `herdr` TUI on the
/// user's default session doesn't pick up lazed's panes by accident —
/// attach on purpose with `herdr --session lazed`.
pub fn session_name() -> String {
    std::env::var("LAZED_HERDR_SESSION").unwrap_or_else(|_| "lazed".into())
}

/// `--session <name>` prefix for every CLI invocation; empty for `default`.
fn session_flag() -> Vec<String> {
    let s = session_name();
    if s.is_empty() || s == "default" {
        vec![]
    } else {
        vec!["--session".into(), s]
    }
}

/// Resolve the herdr binary: `HERDR_BIN` → PATH → known install dirs.
/// The app registers a bundled copy by setting `HERDR_BIN` before it
/// spawns the daemon, so the resolution order stays in one place.
pub fn herdr_bin() -> Result<PathBuf, String> {
    if let Ok(p) = std::env::var("HERDR_BIN") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Ok(p);
        }
    }
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            let c = PathBuf::from(dir).join("herdr");
            if c.is_file() {
                return Ok(c);
            }
        }
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
    Err("herdr_not_installed: herdr binary not found (set HERDR_BIN or install herdr)".into())
}

/// First line of `out` that parses as a JSON object/array — herdr may
/// print human notes around its JSON on stderr, and the CLI can prepend
/// banners on stdout after an upgrade.
pub fn first_json_line(out: &str) -> Option<Value> {
    out.lines()
        .map(str::trim)
        .filter(|l| l.starts_with('{') || l.starts_with('['))
        .find_map(|l| serde_json::from_str::<Value>(l).ok())
}

/// `herdr [--session X] status --json`.
pub fn status() -> Result<Value, String> {
    let bin = herdr_bin()?;
    let mut args = session_flag();
    args.extend(["status".into(), "--json".into()]);
    let out = std::process::Command::new(&bin)
        .args(&args)
        .output()
        .map_err(|e| format!("herdr_exec_failed: {}: {e}", bin.display()))?;
    let text = String::from_utf8_lossy(&out.stdout);
    first_json_line(&text).ok_or_else(|| {
        format!(
            "herdr_bad_status: no JSON from `herdr status --json` ({})",
            String::from_utf8_lossy(&out.stderr).trim()
        )
    })
}

/// Cached socket path — `herdr status` is a process spawn, so resolve once
/// and drop the cache only when a connect fails.
static SOCKET_CACHE: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn socket_path() -> Result<PathBuf, String> {
    if let Some(p) = SOCKET_CACHE.lock().ok().and_then(|g| g.clone()) {
        return Ok(p);
    }
    let st = status()?;
    let p = st
        .pointer("/server/socket")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| "herdr_bad_status: status reported no /server/socket".to_string())?;
    if let Ok(mut g) = SOCKET_CACHE.lock() {
        *g = Some(p.clone());
    }
    Ok(p)
}

fn forget_socket() {
    if let Ok(mut g) = SOCKET_CACHE.lock() {
        *g = None;
    }
}

fn connect() -> Result<UnixStream, String> {
    let path = socket_path()?;
    UnixStream::connect(&path).map_err(|e| {
        forget_socket();
        format!("herdr_not_running: cannot connect {}: {e}", path.display())
    })
}

/// Split a herdr response line into Ok(result) / Err("code: message").
pub fn parse_response(line: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_str(line.trim())
        .map_err(|e| format!("herdr_bad_response: {e}: {}", line.trim()))?;
    if let Some(err) = v.get("error") {
        let code = err.get("code").and_then(Value::as_str).unwrap_or("herdr_error");
        let msg = err.get("message").and_then(Value::as_str).unwrap_or("");
        return Err(format!("{code}: {msg}"));
    }
    Ok(v.get("result").cloned().unwrap_or(Value::Null))
}

fn call_once(method: &str, params: &Value) -> Result<Value, String> {
    let mut sock = connect()?;
    let _ = sock.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = sock.set_write_timeout(Some(Duration::from_secs(5)));
    let req = json!({"id": format!("lazed:{method}"), "method": method, "params": params});
    sock.write_all(serde_json::to_string(&req).map_err(|e| e.to_string())?.as_bytes())
        .and_then(|_| sock.write_all(b"\n"))
        .and_then(|_| sock.flush())
        .map_err(|e| {
            forget_socket();
            format!("herdr_not_running: write failed: {e}")
        })?;
    let mut line = String::new();
    BufReader::new(sock)
        .read_line(&mut line)
        .map_err(|e| format!("herdr_not_running: read failed: {e}"))?;
    if line.trim().is_empty() {
        return Err("herdr_not_running: server closed the connection without a response".into());
    }
    parse_response(&line)
}

/// One herdr API call. A transport failure re-resolves the socket (the
/// server may have restarted on a new path) and retries once; a server
/// `error` response is returned as-is.
pub fn call(method: &str, params: Value) -> Result<Value, String> {
    match call_once(method, &params) {
        Err(e) if e.starts_with("herdr_not_running") => call_once(method, &params),
        r => r,
    }
}

/// `session.snapshot` → the inner `snapshot` object
/// ({workspaces, tabs, panes, layouts, agents, version, protocol}).
pub fn snapshot() -> Result<Value, String> {
    let r = call("session.snapshot", json!({}))?;
    Ok(r.get("snapshot").cloned().unwrap_or(r))
}

/// Non-None when the reachable server should not be trusted blindly.
pub fn compat_warning(st: &Value) -> Option<String> {
    let running = st.pointer("/server/running").and_then(Value::as_bool).unwrap_or(false);
    if !running {
        return None;
    }
    let version = st.pointer("/server/version").and_then(Value::as_str).unwrap_or("?");
    let compatible = st.pointer("/server/compatible").and_then(Value::as_bool).unwrap_or(true);
    let endpoint = st
        .pointer("/server/endpoint_compatible")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if !compatible || !endpoint {
        return Some(format!("herdr {version} reports an incompatible protocol"));
    }
    if version != EXPECTED_VERSION {
        return Some(format!("herdr {version} running, lazed is pinned to {EXPECTED_VERSION}"));
    }
    None
}

// ---------------------------------------------------------------------------
// pane primitives — the calls Step B (herdr-backed terminals) builds on.
// Each returns the herdr ids lazed stores; nothing else about the pane is
// kept on this side.

/// Create a herdr workspace; herdr opens its root tab + pane in `cwd`.
/// Returns (workspace_id, tab_id, pane_id).
#[allow(dead_code)] // wired up by Step B (herdr-backed terminals)
pub fn workspace_create(cwd: &str, label: Option<&str>) -> Result<(String, String, String), String> {
    let r = call("workspace.create", json!({"cwd": cwd, "label": label}))?;
    let ws = str_at(&r, "/workspace/workspace_id")?;
    let tab = str_at(&r, "/tab/tab_id")?;
    let pane = str_at(&r, "/root_pane/pane_id")?;
    Ok((ws, tab, pane))
}

/// New tab in a herdr workspace, with its root pane. Returns (tab_id, pane_id).
#[allow(dead_code)] // wired up by Step B (herdr-backed terminals)
pub fn tab_create(workspace_id: &str, cwd: &str, label: Option<&str>) -> Result<(String, String), String> {
    let r = call(
        "tab.create",
        json!({"workspace_id": workspace_id, "cwd": cwd, "label": label}),
    )?;
    Ok((str_at(&r, "/tab/tab_id")?, str_at(&r, "/root_pane/pane_id")?))
}

/// Split next to `target_pane_id`; the new pane starts a shell in `cwd`.
#[allow(dead_code)] // wired up by Step B (herdr-backed terminals)
pub fn pane_split(target_pane_id: &str, direction: &str, cwd: &str) -> Result<String, String> {
    let r = call(
        "pane.split",
        json!({"target_pane_id": target_pane_id, "direction": direction, "cwd": cwd}),
    )?;
    str_at(&r, "/pane/pane_id")
}

#[allow(dead_code)] // wired up by Step B (herdr-backed terminals)
pub fn pane_close(pane_id: &str) -> Result<(), String> {
    call("pane.close", json!({"pane_id": pane_id})).map(|_| ())
}

/// Type `text` into the pane, then press `keys` (e.g. `["enter"]`).
#[allow(dead_code)] // wired up by Step B (herdr-backed terminals)
pub fn pane_send_input(pane_id: &str, text: &str, keys: &[&str]) -> Result<(), String> {
    call("pane.send_input", json!({"pane_id": pane_id, "text": text, "keys": keys})).map(|_| ())
}

#[allow(dead_code)] // wired up by Step B (herdr-backed terminals)
pub fn pane_get(pane_id: &str) -> Result<Value, String> {
    let r = call("pane.get", json!({"pane_id": pane_id}))?;
    Ok(r.get("pane").cloned().unwrap_or(r))
}

#[allow(dead_code)] // wired up by Step B (herdr-backed terminals)
fn str_at(v: &Value, pointer: &str) -> Result<String, String> {
    v.pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("herdr_bad_response: missing {pointer} in {v}"))
}

// ---------------------------------------------------------------------------
// event bridge

/// Global (session-wide) event types. Per-pane `pane.agent_status_changed`
/// subscriptions are added for every pane in the snapshot at connect time;
/// a `pane_created` afterwards restarts the stream so the new pane is
/// covered (herdr closes a stream that receives a second subscribe).
pub const GLOBAL_SUBS: &[&str] = &[
    "workspace.created",
    "workspace.closed",
    "workspace.focused",
    "workspace.renamed",
    "tab.created",
    "tab.closed",
    "tab.focused",
    "tab.renamed",
    "pane.created",
    "pane.closed",
    "pane.updated",
    "pane.focused",
    "pane.exited",
    "pane.agent_detected",
    "layout.updated",
];

pub fn subscribe_request(pane_ids: &[String]) -> Value {
    let mut subs: Vec<Value> = GLOBAL_SUBS.iter().map(|t| json!({"type": t})).collect();
    for id in pane_ids {
        subs.push(json!({"type": "pane.agent_status_changed", "pane_id": id}));
    }
    json!({"id": "lazed:events", "method": "events.subscribe", "params": {"subscriptions": subs}})
}

/// Bus name for a herdr event line, or None for non-event lines (the
/// subscription ack, stray responses).
pub fn bridge_event_name(line: &Value) -> Option<String> {
    line.get("event").and_then(Value::as_str).map(|e| format!("herdr.{e}"))
}

pub fn pane_ids_of(snapshot: &Value) -> Vec<String> {
    snapshot
        .get("panes")
        .and_then(Value::as_array)
        .map(|ps| {
            ps.iter()
                .filter_map(|p| p.get("pane_id").and_then(Value::as_str).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

static CONNECTED: AtomicBool = AtomicBool::new(false);
static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);

pub fn bridge_connected() -> bool {
    CONNECTED.load(Ordering::Relaxed)
}

fn set_error(e: Option<String>) {
    if let Ok(mut g) = LAST_ERROR.lock() {
        *g = e;
    }
}

/// `herdr.status` API body: discovery + bridge state in one object.
pub fn status_json() -> Value {
    let bin = herdr_bin().ok().map(|p| p.display().to_string());
    let st = status();
    let (server, warning) = match &st {
        Ok(v) => (v.get("server").cloned().unwrap_or(Value::Null), compat_warning(v)),
        Err(_) => (Value::Null, None),
    };
    json!({
        "installed": bin.is_some(),
        "binary": bin,
        "session": session_name(),
        "expected_version": EXPECTED_VERSION,
        "running": server.get("running").and_then(Value::as_bool).unwrap_or(false),
        "version": server.get("version").cloned().unwrap_or(Value::Null),
        "socket": server.get("socket").cloned().unwrap_or(Value::Null),
        "compat_warning": warning,
        "bridge_connected": bridge_connected(),
        "error": st.err().or_else(|| LAST_ERROR.lock().ok().and_then(|g| g.clone())),
    })
}

/// Run the event bridge forever. One connection at a time; after a drop the
/// loop backs off (1s → 5s) and re-subscribes, publishing
/// `herdr.disconnected` / `herdr.connected {snapshot}` around the gap.
pub fn spawn_bridge(session: Arc<Mutex<Session>>) {
    std::thread::spawn(move || {
        let mut backoff = Duration::from_secs(1);
        loop {
            match run_stream(&session) {
                StreamEnd::Restart => {
                    // pane_created: reconnect right away to cover the new pane
                    backoff = Duration::from_secs(1);
                    continue;
                }
                StreamEnd::Failed(e) => {
                    if CONNECTED.swap(false, Ordering::Relaxed) {
                        crate::term::lock(&session)
                            .broadcast_event("herdr.disconnected", json!({"error": e}));
                    }
                    set_error(Some(e));
                    std::thread::sleep(backoff);
                    backoff = (backoff * 2).min(Duration::from_secs(5));
                }
            }
        }
    });
}

enum StreamEnd {
    Restart,
    Failed(String),
}

fn run_stream(session: &Arc<Mutex<Session>>) -> StreamEnd {
    let snap = match snapshot() {
        Ok(s) => s,
        Err(e) => return StreamEnd::Failed(e),
    };
    let pane_ids = pane_ids_of(&snap);
    let mut sock = match connect() {
        Ok(s) => s,
        Err(e) => return StreamEnd::Failed(e),
    };
    let req = serde_json::to_string(&subscribe_request(&pane_ids)).unwrap_or_default();
    if let Err(e) = sock
        .write_all(req.as_bytes())
        .and_then(|_| sock.write_all(b"\n"))
        .and_then(|_| sock.flush())
    {
        return StreamEnd::Failed(format!("herdr_not_running: subscribe write failed: {e}"));
    }
    let mut reader = BufReader::new(sock);
    let mut line = String::new();
    // first line is the subscription ack — anything else is a refusal
    if let Err(e) = reader.read_line(&mut line) {
        return StreamEnd::Failed(format!("herdr_not_running: subscribe read failed: {e}"));
    }
    if let Err(e) = parse_response(&line) {
        return StreamEnd::Failed(format!("herdr_subscribe_refused: {e}"));
    }
    if !CONNECTED.swap(true, Ordering::Relaxed) {
        set_error(None);
    }
    // always announce — a Restart-driven reconnect carries a fresh snapshot
    // too, and that is exactly when subscribers should reconcile pane state
    crate::term::lock(session).broadcast_event(
        "herdr.connected",
        json!({"session": session_name(), "snapshot": snap}),
    );
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => return StreamEnd::Failed("herdr_not_running: event stream closed".into()),
            Err(e) => return StreamEnd::Failed(format!("herdr_not_running: event read failed: {e}")),
            Ok(_) => {}
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(trimmed) else { continue };
        let Some(name) = bridge_event_name(&v) else { continue };
        let data = v.get("data").cloned().unwrap_or(Value::Null);
        let is_pane_created = name == "herdr.pane_created";
        crate::term::lock(session).broadcast_event(&name, data);
        if is_pane_created {
            return StreamEnd::Restart;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_json_line_skips_banners() {
        let out = "herdr 0.9.0\nupdate available!\n{\"server\":{\"running\":false}}\n";
        let v = first_json_line(out).unwrap();
        assert_eq!(v.pointer("/server/running"), Some(&json!(false)));
        assert!(first_json_line("no json here").is_none());
    }

    #[test]
    fn parse_response_result_and_error() {
        assert_eq!(
            parse_response("{\"id\":\"x\",\"result\":{\"type\":\"ok\"}}").unwrap(),
            json!({"type": "ok"})
        );
        let e = parse_response(
            "{\"id\":\"\",\"error\":{\"code\":\"server_not_running\",\"message\":\"no server\"}}",
        )
        .unwrap_err();
        assert_eq!(e, "server_not_running: no server");
        assert!(parse_response("garbage").unwrap_err().starts_with("herdr_bad_response"));
    }

    #[test]
    fn subscribe_request_covers_globals_and_panes() {
        let req = subscribe_request(&["w1:p1".into(), "w1:p2".into()]);
        assert_eq!(req["method"], "events.subscribe");
        let subs = req["params"]["subscriptions"].as_array().unwrap();
        assert_eq!(subs.len(), GLOBAL_SUBS.len() + 2);
        assert_eq!(subs[GLOBAL_SUBS.len()], json!({"type": "pane.agent_status_changed", "pane_id": "w1:p1"}));
        assert!(subs.iter().any(|s| s["type"] == "pane.created"));
    }

    #[test]
    fn bridge_event_name_prefixes_and_ignores_acks() {
        assert_eq!(
            bridge_event_name(&json!({"event": "pane_exited", "data": {}})).as_deref(),
            Some("herdr.pane_exited")
        );
        assert!(bridge_event_name(&json!({"id": "sub", "result": {"type": "subscription_started"}})).is_none());
    }

    #[test]
    fn pane_ids_of_snapshot() {
        let snap = json!({"panes": [{"pane_id": "w1:p1"}, {"pane_id": "w1:p3"}], "tabs": []});
        assert_eq!(pane_ids_of(&snap), vec!["w1:p1", "w1:p3"]);
        assert!(pane_ids_of(&json!({})).is_empty());
    }

    #[test]
    fn compat_warning_rules() {
        let ok = json!({"server": {"running": true, "version": EXPECTED_VERSION, "compatible": true, "endpoint_compatible": true}});
        assert!(compat_warning(&ok).is_none());
        let down = json!({"server": {"running": false, "version": null}});
        assert!(compat_warning(&down).is_none());
        let other = json!({"server": {"running": true, "version": "0.10.0", "compatible": true, "endpoint_compatible": true}});
        assert!(compat_warning(&other).unwrap().contains("pinned"));
        let incompat = json!({"server": {"running": true, "version": EXPECTED_VERSION, "compatible": false, "endpoint_compatible": true}});
        assert!(compat_warning(&incompat).unwrap().contains("incompatible"));
    }

    #[test]
    fn session_flag_omits_default() {
        // env-dependent: only assert the shape of the two branches
        let flag = session_flag();
        assert!(flag.is_empty() || (flag.len() == 2 && flag[0] == "--session"));
    }
}
