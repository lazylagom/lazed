//! Unix socket server: NDJSON request/response + pushed events. One
//! connection = one client channel (mpsc outbox → writer).
//!
//! Path C: this daemon serves the organization layer (groups, projects,
//! tasks, inbox) and re-broadcasts herdr's events.
//! Terminal bytes never pass through here — the app streams frames from
//! herdr directly, and tab/pane control reaches herdr through the
//! `herdr.call` passthrough.
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::herdr;
use crate::session::{lock, Session};
use crate::state;

pub(crate) type Shared = Arc<Mutex<Session>>;

/// Slow readers are disconnected rather than losing events or blocking producers.
#[derive(Clone)]
pub struct ClientSender {
    tx: SyncSender<Arc<str>>,
    queued: Arc<AtomicUsize>,
    socket: Arc<UnixStream>,
}
impl ClientSender {
    pub fn send(&self, value: Value) -> Result<(), ()> {
        let mut line = serde_json::to_string(&value).map_err(|_| ())?;
        line.push('\n');
        self.send_line(line.into())
    }

    /// Pre-serialized JSON line (trailing newline included) — broadcast
    /// paths serialize once and share one buffer across subscribers.
    pub fn send_line(&self, line: Arc<str>) -> Result<(), ()> {
        const MAX_BYTES: usize = 8 * 1024 * 1024;
        let size = line.len();
        if self.queued.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
            n.checked_add(size).filter(|n| *n <= MAX_BYTES)
        }).is_err() {
            let _ = self.socket.shutdown(std::net::Shutdown::Both);
            return Err(());
        }
        if self.tx.try_send(line).is_err() {
            self.queued.fetch_sub(size, Ordering::Relaxed);
            let _ = self.socket.shutdown(std::net::Shutdown::Both);
            return Err(());
        }
        Ok(())
    }
}

/// Unix seconds when this daemon process started (first touched in `run`).
static STARTED_AT: std::sync::LazyLock<u64> = std::sync::LazyLock::new(|| {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
});

/// (dev, ino, mtime) of our own binary — a later mismatch means the file
/// on disk was replaced and a restart would pick up a newer daemon.
fn binary_stamp() -> Option<(u64, u64, std::time::SystemTime)> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(std::env::current_exe().ok()?).ok()?;
    Some((meta.dev(), meta.ino(), meta.modified().ok()?))
}
static BINARY_STAMP: std::sync::LazyLock<Option<(u64, u64, std::time::SystemTime)>> =
    std::sync::LazyLock::new(binary_stamp);

pub fn run() -> std::io::Result<()> {
    let _ = *STARTED_AT;
    let _ = *BINARY_STAMP;
    state::ensure_dir()?;
    let path = state::sock_path();
    if path.exists() {
        // stale socket from a dead server — refuse only if someone answers
        if UnixStream::connect(&path).is_ok() {
            eprintln!("lazed: server already running ({})", path.display());
            std::process::exit(1);
        }
        let _ = std::fs::remove_file(&path);
    }
    let _ = std::fs::write(state::pid_path(), std::process::id().to_string());

    let session: Shared = Arc::new(Mutex::new(Session::new()));
    lock(&session).restore();

    // periodic persist
    {
        let s = session.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(30));
            if let Err(error) = lock(&s).persist() { eprintln!("{error}"); }
        });
    }

    // herdr event bridge — re-broadcasts execution-layer events as
    // `herdr.*`; keeps retrying while herdr is down (degraded mode)
    herdr::spawn_bridge(session.clone());

    let listener = UnixListener::bind(&path)?;
    eprintln!("lazed: listening on {}", path.display());
    for conn in listener.incoming() {
        match conn {
            Ok(stream) => {
                let s = session.clone();
                std::thread::spawn(move || handle_conn(stream, s));
            }
            Err(e) => eprintln!("lazed: accept failed: {e}"),
        }
    }
    Ok(())
}

fn handle_conn(stream: UnixStream, session: Shared) {
    let writer_stream = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let socket = match stream.try_clone() { Ok(s) => Arc::new(s), Err(_) => return };
    let (out, rx) = sync_channel::<Arc<str>>(128);
    let queued = Arc::new(AtomicUsize::new(0));
    let tx = ClientSender { tx: out, queued: queued.clone(), socket: socket.clone() };
    // writer thread: every outbox value becomes one JSON line on the socket
    std::thread::spawn(move || {
        let mut w = writer_stream;
        let _ = w.set_write_timeout(Some(std::time::Duration::from_secs(5)));
        for line in rx {
            let result = w.write_all(line.as_bytes());
            queued.fetch_sub(line.len(), Ordering::Relaxed);
            if result.is_err() {
                break;
            }
            let _ = w.flush();
        }
        let _ = socket.shutdown(std::net::Shutdown::Both);
    });

    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(m) = msg.get("method").and_then(Value::as_str) else { continue };
        let id = msg.get("id").cloned();
        let params = msg.get("params").cloned().unwrap_or(json!({}));
        // Anything that talks to herdr or git runs without the session lock:
        // one slow herdr call must not wedge every other client.
        let res = if m == "session.snapshot" {
            session_snapshot(&session)
        } else if m == "project.create" {
            project_create(&session, &params)
        } else if m == "project.close" {
            project_close(&session, &params)
        } else if m == "workspace.rename" {
            workspace_rename(&session, &params)
        } else if m.starts_with("herdr.") {
            herdr_call(m, &params)
        } else if m.starts_with("task.") {
            crate::tasks::handle(&session, m, &params).map(crate::tasks::public)
        } else if m.starts_with("inbox.") {
            let mut s = lock(&session);
            crate::inbox::handle(&mut s, m, &params)
        } else if m.starts_with("todo.") {
            let mut s = lock(&session);
            crate::todo::handle(&mut s, m, &params)
        } else {
            let mut s = lock(&session);
            dispatch(&mut s, m, &params, &tx)
        };
        match res {
            Ok(res) => {
                if let Some(id) = id {
                    let _ = tx.send(json!({"id": id, "result": res}));
                }
            }
            Err(e) => {
                if let Some(id) = id {
                    let _ = tx.send(json!({"id": id, "error": e}));
                }
            }
        }
    }
    // connection dropped — forget its event subscription
    let mut s = lock(&session);
    s.event_subs.retain(|out| !Arc::ptr_eq(&out.queued, &tx.queued));
}

/// Lock-held, IO-free methods.
fn dispatch(s: &mut Session, method: &str, p: &Value, tx: &ClientSender) -> Result<Value, String> {
    let str_of = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or("");
    let opt = |k: &str| {
        let v = str_of(k).to_string();
        if v.is_empty() { None } else { Some(v) }
    };
    match method {
        "session.status" => {
            let exe = std::env::current_exe().ok().and_then(|p| p.canonicalize().ok());
            let binary_updated = matches!(
                (*BINARY_STAMP, binary_stamp()),
                (Some(started), Some(current)) if started != current
            );
            Ok(json!({
                "running": true,
                "version": env!("CARGO_PKG_VERSION"),
                "capabilities": ["herdr.overlay.v1", "task.v1", "inbox.v1", "todo.v1", "server.restart.v1", "server.stop_if_empty.v1"],
                "exe": exe.map(|p| p.to_string_lossy().to_string()),
                "pid": std::process::id(),
                "started_at": *STARTED_AT,
                "binary_updated": binary_updated,
                "herdr": {
                    "session": herdr::session_name(),
                    "bridge_connected": herdr::bridge_connected(),
                },
                "workspaces": s.workspaces.len(),
                "projects": s.projects.len(),
            }))
        }
        "session.persist" => {
            s.persist()?;
            Ok(json!({"ok": true}))
        }
        "server.stop_if_empty" => {
            s.persist()?;
            std::process::exit(0);
        }
        "server.stop" => {
            s.persist()?;
            // reply first, exit after the response line is flushed
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_millis(50));
                std::process::exit(0);
            });
            Ok(json!({"ok": true}))
        }

        "project.list" => Ok(json!(
            s.projects.keys().map(|id| s.project_json(id)).collect::<Vec<_>>()
        )),
        "project.focus" => {
            let id = str_of("project_id").to_string();
            if !s.projects.contains_key(&id) {
                return Err(format!("no project {id}"));
            }
            s.focused_project_id = Some(id.clone());
            s.broadcast_event("project.focused", json!({"project_id": id}));
            Ok(json!({"ok": true}))
        }
        "project.rename" => {
            let id = str_of("project_id");
            let label = opt("label");
            let proj = s
                .projects
                .get_mut(id)
                .ok_or_else(|| format!("no project {id}"))?;
            proj.label = label;
            s.broadcast_event("project.updated", json!({"project_id": id}));
            Ok(json!({"ok": true}))
        }

        "group.create" => Ok(s.create_group(opt("label"))),
        "group.list" => Ok(json!(
            s.groups.iter().map(|g| s.group_json(g)).collect::<Vec<_>>()
        )),
        "group.rename" => {
            s.rename_group(str_of("group_id"), opt("label"))?;
            Ok(json!({"ok": true}))
        }
        "group.remove" => {
            s.remove_group(str_of("group_id"))?;
            Ok(json!({"ok": true}))
        }
        "group.assign" => {
            let group = opt("group_id");
            s.assign_project(str_of("project_id"), group.as_deref())?;
            Ok(json!({"ok": true}))
        }

        "workspace.list" => Ok(json!(
            s.workspaces.keys().map(|id| s.workspace_json(id)).collect::<Vec<_>>()
        )),
        "agent.specs" => Ok(crate::specs::AgentRegistry::load().json()),
        "events.subscribe" => {
            s.event_subs.retain(|out| !Arc::ptr_eq(&out.queued, &tx.queued));
            s.event_subs.push(tx.clone());
            Ok(json!({"subscribed": true}))
        }

        other => Err(format!("unknown method {other}")),
    }
}

/// `herdr.*` passthrough: `herdr.status` (adapter + server health),
/// `herdr.snapshot`, and `herdr.call {method, params}` for tab/pane control
/// and anything else the app or a script needs from herdr.
fn herdr_call(method: &str, p: &Value) -> Result<Value, String> {
    match method {
        "herdr.status" => Ok(herdr::status_json()),
        "herdr.snapshot" => herdr::snapshot(),
        "herdr.call" => {
            let inner = p
                .get("method")
                .and_then(Value::as_str)
                .ok_or("herdr.call needs method")?;
            let params = p.get("params").cloned().unwrap_or_else(|| json!({}));
            herdr::call(inner, params)
        }
        other => Err(format!("unknown method {other}")),
    }
}

/// Fetch herdr's snapshot (no lock), reconcile annotations against it,
/// adopt workspaces opened outside lazed, then build the merged snapshot.
/// herdr down → degraded snapshot from annotations alone.
fn session_snapshot(session: &Shared) -> Result<Value, String> {
    let herdr_snap = herdr::snapshot().ok();
    if let Some(h) = &herdr_snap {
        reconcile(session, h);
    }
    Ok(lock(session).snapshot_json(herdr_snap.as_ref()))
}

/// Reconcile + adopt. git IO for unknown workspaces happens between the
/// two lock scopes.
pub(crate) fn reconcile(session: &Shared, herdr_snap: &Value) {
    let unknown = lock(session).reconcile(herdr_snap);
    if unknown.is_empty() {
        return;
    }
    let resolved: Vec<_> = unknown
        .into_iter()
        .map(|(id, cwd)| {
            let (root, key) = Session::resolve_repo(&cwd);
            let branch = Session::detect_branch(&cwd);
            let checkout = crate::repo::resolve(&cwd).map(|r| r.checkout).unwrap_or(cwd);
            (id, checkout, root, key, branch)
        })
        .collect();
    let mut s = lock(session);
    for (id, cwd, root, key, branch) in resolved {
        if let Err(e) = s.adopt_workspace(&id, &cwd, &root, &key, branch) {
            eprintln!("lazed: adopt {id}: {e}");
        }
    }
}

/// `project.create {cwd, label?, group_id?}` — file the repo as a project
/// and open its main checkout as a herdr workspace. With herdr down the
/// project is still registered (degraded); the workspace opens on the
/// first snapshot after herdr is back, via adoption of whatever the user
/// opens, or explicitly with another project.create.
fn project_create(session: &Shared, p: &Value) -> Result<Value, String> {
    let cwd = p.get("cwd").and_then(Value::as_str).ok_or("project.create needs cwd")?;
    let label = p.get("label").and_then(Value::as_str).map(String::from);
    let group = p.get("group_id").and_then(Value::as_str).filter(|g| !g.is_empty()).map(String::from);
    let (repo_root, repo_key) = Session::resolve_repo(cwd);
    let branch = Session::detect_branch(&repo_root);
    let project_id = lock(session).ensure_project(&repo_root, &repo_key, label, group.as_deref())?;
    let has_main = lock(session).main_workspace(&project_id).is_some();
    let mut opened = Value::Null;
    if !has_main {
        match herdr::workspace_create(&repo_root, project_label(&repo_root).as_deref()) {
            Ok((ws, tab, pane)) => {
                let mut s = lock(session);
                s.register_workspace(&ws, &project_id, &repo_root, None, branch, true)?;
                opened = json!({"workspace_id": ws, "tab_id": tab, "pane_id": pane});
            }
            Err(e) if e.starts_with("herdr_not_running") || e.starts_with("herdr_not_installed") => {
                opened = json!({"error": e});
            }
            Err(e) => return Err(e),
        }
    }
    // the skill is a convenience — a failed install never fails the import
    let init = if p.get("init_skills").and_then(Value::as_bool) == Some(true) {
        crate::crew::init(std::path::Path::new(&repo_root), false)
            .unwrap_or_else(|e| json!({"error": e}))
    } else {
        Value::Null
    };
    let s = lock(session);
    Ok(json!({
        "project": s.project_json(&project_id),
        "workspace": s.main_workspace(&project_id).map(|w| s.workspace_json(&w.id)),
        "opened": opened,
        "init": init,
    }))
}

fn project_label(repo_root: &str) -> Option<String> {
    std::path::Path::new(repo_root)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
}

/// `project.close` — drop the project and close its herdr workspaces
/// (panes included). Checkouts on disk are untouched.
fn project_close(session: &Shared, p: &Value) -> Result<Value, String> {
    let id = p.get("project_id").and_then(Value::as_str).unwrap_or("");
    let workspaces = lock(session).close_project(id)?;
    let mut failed = Vec::new();
    for ws in &workspaces {
        if let Err(e) = herdr::call("workspace.close", json!({"workspace_id": ws})) {
            failed.push(json!({"workspace_id": ws, "error": e}));
        }
    }
    Ok(json!({"ok": true, "closed": workspaces, "herdr_failed": failed}))
}

/// `workspace.rename` — the label lives on both sides: herdr shows it in
/// its TUI/tabs, lazed keeps it for degraded mode.
fn workspace_rename(session: &Shared, p: &Value) -> Result<Value, String> {
    let id = p.get("workspace_id").and_then(Value::as_str).unwrap_or("");
    let label = p.get("label").and_then(Value::as_str).unwrap_or("").to_string();
    {
        let mut s = lock(session);
        let ws = s
            .workspaces
            .get_mut(id)
            .ok_or_else(|| format!("no workspace {id}"))?;
        ws.label = if label.is_empty() { None } else { Some(label.clone()) };
        s.broadcast_event("workspace.updated", json!({"workspace_id": id}));
    }
    if !label.is_empty() {
        let _ = herdr::call("workspace.rename", json!({"workspace_id": id, "label": label}));
    }
    Ok(json!({"ok": true}))
}

#[cfg(test)]
mod reliability_tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn full_outbox_disconnects_instead_of_growing() {
        let (socket, mut peer) = UnixStream::pair().unwrap();
        let (tx, rx) = sync_channel(1);
        let sender = ClientSender { tx, queued: Arc::new(AtomicUsize::new(0)), socket: Arc::new(socket) };
        sender.send(json!({"n": 1})).unwrap();
        assert!(sender.send(json!({"n": 2})).is_err());
        assert_eq!(sender.queued.load(Ordering::Relaxed), rx.recv().unwrap().len());
        peer.set_read_timeout(Some(std::time::Duration::from_secs(1))).unwrap();
        assert_eq!(peer.read(&mut [0; 1]).unwrap(), 0);
    }

    #[test]
    fn outbox_rejects_oversized_frame() {
        let (socket, _peer) = UnixStream::pair().unwrap();
        let (tx, rx) = sync_channel(1);
        let sender = ClientSender { tx, queued: Arc::new(AtomicUsize::new(0)), socket: Arc::new(socket) };
        assert!(sender.send(json!("x".repeat(8 * 1024 * 1024))).is_err());
        assert!(rx.try_recv().is_err());
        assert_eq!(sender.queued.load(Ordering::Relaxed), 0);
    }

}
