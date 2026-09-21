//! Unix socket server: NDJSON request/response + pushed events + attach
//! streams. One connection = one client channel (mpsc outbox → writer).
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::session::Session;
use crate::state;
use crate::term::{lock, PtyTerm};

pub(crate) type Shared = Arc<Mutex<Session>>;

/// Slow readers are disconnected rather than losing diff frames or blocking producers.
#[derive(Clone)]
pub struct ClientSender {
    tx: SyncSender<String>,
    queued: Arc<AtomicUsize>,
    socket: Arc<UnixStream>,
}
impl ClientSender {
    pub fn send(&self, value: Value) -> Result<(), ()> {
        const MAX_BYTES: usize = 8 * 1024 * 1024;
        let mut line = serde_json::to_string(&value).map_err(|_| ())?;
        line.push('\n');
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

pub fn run() -> std::io::Result<()> {
    let _ = *STARTED_AT;
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

    // trashed worktree checkouts a previous run never finished deleting
    sweep_worktree_trash();

    // periodic persist
    {
        let s = session.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(30));
            if let Err(error) = lock(&s).persist() { eprintln!("{error}"); }
        });
    }

    // agent detection tick
    {
        let s = session.clone();
        std::thread::spawn(move || Session::agent_watch(s));
    }

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

static CONN_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn handle_conn(stream: UnixStream, session: Shared) {
    let conn_id = CONN_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let writer_stream = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let socket = match stream.try_clone() { Ok(s) => Arc::new(s), Err(_) => return };
    let (out, rx) = sync_channel::<String>(128);
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

    let mut attached: Option<String> = None;
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
        // attach-mode commands use {"type": ...}; api calls use {"method": ...}
        if let Some(m) = msg.get("method").and_then(Value::as_str) {
            let id = msg.get("id").cloned();
            let params = msg.get("params").cloned().unwrap_or(json!({}));
            // agent.wait is a long poll — it must not hold the session lock
            // while parked, or one unreachable wait wedges every client.
            let res = if matches!(m, "agent.start" | "agent.prompt" | "agent.get" | "agent.wait" | "agent.read" | "agent.list" | "agent.send_keys" | "agent.release") {
                crate::named::handle(&session, m, &params)
            } else if m == "agent.report" {
                crate::control::handle(&session, m, &params)
            } else if m == "workspace.create" {
                workspace_create(&session, &params)
            } else if m == "workspace.remove" {
                workspace_remove(&session, &params)
            } else if m == "worktree.list" {
                worktree_list(&params)
            } else if m.starts_with("task.") {
                crate::tasks::handle(&session, m, &params).map(crate::tasks::public)
            } else if m.starts_with("inbox.") {
                let mut s = lock(&session);
                crate::inbox::handle(&mut s, m, &params)
            } else {
                let mut jobs = Vec::new();
                let mut r = {
                    let mut s = lock(&session);
                    dispatch(&mut s, m, &params, conn_id, &tx, &mut attached, &mut jobs)
                };
                // PTY writes after the session lock drops — a child that
                // stalls on input must not freeze other clients
                for (t, bytes) in jobs {
                    // every job is injected input — snap the view to the
                    // live edge first so input never lands invisibly while
                    // the pane is scrolled up
                    lock(&t).scroll_to(0);
                    if let Err(e) = term_write(&t, &bytes) { r = Err(e); break; }
                }
                r
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
        } else if let Some(t) = msg.get("type").and_then(Value::as_str) {
            let _ = term_command(&session, t, &msg, conn_id, &mut attached);
        }
    }
    // connection dropped — detach from any terminal
    let mut s = lock(&session);
    s.event_subs.retain(|out| !Arc::ptr_eq(&out.queued, &tx.queued));
    if let Some(tid) = attached {
        if let Ok(t) = s.get_terminal(&tid) {
            lock(&t).unsubscribe(conn_id);
        }
    }
}

/// Write bytes to a terminal's PTY input. The term lock is held only long
/// enough to clone the writer handle — a child that stops draining input
/// stalls this write but never the renderer or the session.
pub(crate) fn term_write(t: &Arc<Mutex<PtyTerm>>, bytes: &[u8]) -> Result<(), String> {
    let writer = lock(t).writer();
    let mut w = lock(&writer);
    {
        let mut g = lock(t);
        if g.maintenance { return Err("workspace removal in progress".into()); }
        g.input_seq = g.input_seq.wrapping_add(1);
        g.history_reading = false;
    }
    w.write_all(bytes).and_then(|_| w.flush()).map_err(|e| format!("pty_write_failed: {e}"))
}

fn dispatch(
    s: &mut Session,
    method: &str,
    p: &Value,
    conn_id: u64,
    tx: &ClientSender,
    attached: &mut Option<String>,
    jobs: &mut Vec<(Arc<Mutex<PtyTerm>>, Vec<u8>)>,
) -> Result<Value, String> {
    let str_of = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or("");
    match method {
        "session.status" => {
            let exe = std::env::current_exe().ok().and_then(|p| p.canonicalize().ok());
            // the binary on disk was rebuilt after this process started —
            // the running daemon is stale until restarted
            let binary_updated = exe
                .as_ref()
                .and_then(|p| std::fs::metadata(p).ok())
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() > *STARTED_AT)
                .unwrap_or(false);
            Ok(json!({
                "running": true,
                "version": env!("CARGO_PKG_VERSION"),
                "capabilities": ["agent.lifecycle.v1", "task.v1", "agent.names.v1", "pane.context.v1", "inbox.v1", "agent.busy_prompt.v1", "agent.history.v1", "server.restart.v1"],
                "exe": exe.map(|p| p.to_string_lossy().to_string()),
                "pid": std::process::id(),
                "started_at": *STARTED_AT,
                "binary_updated": binary_updated,
                "terms": s.terminals.len(),
                "projects": s.projects.len(),
            }))
        }
        "session.snapshot" => Ok(s.snapshot()),
        "session.persist" => {
            s.persist()?;
            Ok(json!({"ok": true}))
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

        "project.create" => {
            let cwd = str_of("cwd");
            let label = p.get("label").and_then(Value::as_str).map(String::from);
            let group = str_of("group_id");
            s.create_project(cwd, label, if group.is_empty() { None } else { Some(group) })
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
            let label = str_of("label").to_string();
            let proj = s
                .projects
                .get_mut(id)
                .ok_or_else(|| format!("no project {id}"))?;
            proj.label = if label.is_empty() { None } else { Some(label) };
            Ok(json!({"ok": true}))
        }
        "project.close" => {
            s.close_project(str_of("project_id"))?;
            Ok(json!({"ok": true}))
        }

        "group.create" => {
            let label = p.get("label").and_then(Value::as_str).map(String::from);
            Ok(s.create_group(label))
        }
        "group.list" => Ok(json!(
            s.groups.iter().map(|g| s.group_json(g)).collect::<Vec<_>>()
        )),
        "group.rename" => {
            let label = str_of("label").to_string();
            s.rename_group(
                str_of("group_id"),
                if label.is_empty() { None } else { Some(label) },
            )?;
            Ok(json!({"ok": true}))
        }
        "group.remove" => {
            s.remove_group(str_of("group_id"))?;
            Ok(json!({"ok": true}))
        }
        "group.assign" => {
            let group = str_of("group_id");
            s.assign_project(
                str_of("project_id"),
                if group.is_empty() { None } else { Some(group) },
            )?;
            Ok(json!({"ok": true}))
        }

        // New pane. Placement by ancestry: `tab_id` appends a pane to that
        // tab (split row); `workspace_id` opens a fresh tab in the
        // workspace; `project_id` opens a fresh tab in the project's main
        // workspace.
        "pane.get" => {
            s.get_terminal(str_of("term_id"))?;
            Ok(s.terminal_json(str_of("term_id")))
        }
        "pane.split" => {
            let caller = s.get_terminal(str_of("term_id"))?;
            let (tab, _, _) = s.term_parents(str_of("term_id"));
            let tab = tab.ok_or("caller has no tab")?;
            let (cwd, cols, rows, kind) = {
                let caller = lock(&caller);
                let cwd = if str_of("cwd").is_empty() { caller.cwd.clone() } else { str_of("cwd").to_string() };
                (cwd, caller.cols(), caller.rows(), caller.kind.clone())
            };
            let t = s.create_terminal(&tab, &cwd, "", None, &kind, cols, rows)?;
            let tid = lock(&t).id.clone();
            Ok(json!({"pane": s.terminal_json(&tid)}))
        }
        "terminal.create" => {
            let tab_id = str_of("tab_id").to_string();
            let workspace_id = str_of("workspace_id").to_string();
            let project_id = str_of("project_id").to_string();
            let command = str_of("command").to_string();
            let label = p.get("label").and_then(Value::as_str).map(String::from);
            let cols = p.get("cols").and_then(Value::as_u64).unwrap_or(80) as usize;
            let rows = p.get("rows").and_then(Value::as_u64).unwrap_or(24) as usize;
            crate::term::validate_size(cols, rows)?;
            // resolve the target tab — open a fresh one when the caller
            // named a workspace or project instead of a tab
            let tid = if !tab_id.is_empty() {
                tab_id
            } else {
                let ws_id = if !workspace_id.is_empty() {
                    workspace_id
                } else {
                    let pid = if project_id.is_empty() {
                        return Err(
                            "terminal.create needs tab_id, workspace_id, or project_id".into(),
                        );
                    } else {
                        project_id
                    };
                    s.main_workspace(&pid)
                        .map(|w| w.id.clone())
                        .ok_or_else(|| format!("project {pid} has no workspace"))?
                };
                s.create_tab(&ws_id, label.clone(), &command)?
                    .get("tab_id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .ok_or("tab.create returned no tab_id")?
            };
            if str_of("tab_id").is_empty() {
                // the fresh tab already carries its first pane
                let pid = s
                    .tabs
                    .get(&tid)
                    .and_then(|t| t.panes.first().cloned())
                    .ok_or("new tab has no pane")?;
                let term = s.get_terminal(&pid)?;
                lock(&term).resize(cols, rows)?;
                return Ok(s.terminal_json(&pid));
            }
            let dir = {
                let cwd = str_of("cwd");
                if cwd.is_empty() {
                    s.tabs
                        .get(&tid)
                        .and_then(|t| s.workspaces.get(&t.workspace_id))
                        .map(|w| w.path.clone())
                        .unwrap_or_else(|| ".".into())
                } else {
                    cwd.to_string()
                }
            };
            let kind = s
                .tabs
                .get(&tid)
                .and_then(|t| s.workspaces.get(&t.workspace_id))
                .map(|w| if w.is_main { "plain" } else { "worktree" })
                .unwrap_or("plain")
                .to_string();
            let t = s.create_terminal(&tid, &dir, &command, label, &kind, cols, rows)?;
            let term_id = lock(&t).id.clone();
            Ok(s.terminal_json(&term_id))
        }
        "terminal.list" => Ok(json!(
            s.terminals.keys().map(|id| s.terminal_json(id)).collect::<Vec<_>>()
        )),
        "terminal.close" => {
            s.close_terminal(str_of("term_id"))?;
            Ok(json!({"ok": true}))
        }
        "terminal.input" => {
            let t = s.get_terminal(str_of("term_id"))?;
            lock(&t).user_scrolled = false;
            jobs.push((t, str_of("text").as_bytes().to_vec()));
            Ok(json!({"ok": true}))
        }
        "terminal.send_keys" => {
            let t = s.get_terminal(str_of("term_id"))?;
            let keys = p
                .get("keys")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let mut out = String::new();
            for k in keys.iter().filter_map(Value::as_str) {
                out.push_str(match k {
                    "enter" | "return" => "\r",
                    "tab" => "\t",
                    "escape" | "esc" => "\x1b",
                    "backspace" => "\x7f",
                    "space" => " ",
                    "up" => "\x1bOA",
                    "down" => "\x1bOB",
                    "right" => "\x1bOC",
                    "left" => "\x1bOD",
                    other => other,
                });
            }
            jobs.push((t, out.into_bytes()));
            Ok(json!({"ok": true}))
        }
        "terminal.read" => {
            let t = s.get_terminal(str_of("term_id"))?;
            let lines = p.get("lines").and_then(Value::as_u64).unwrap_or(40) as usize;
            Ok(json!({"text": lock(&t).screen_text(lines)}))
        }
        "terminal.resize" => {
            let t = s.get_terminal(str_of("term_id"))?;
            let cols = p.get("cols").and_then(Value::as_u64).unwrap_or(80) as usize;
            let rows = p.get("rows").and_then(Value::as_u64).unwrap_or(24) as usize;
            lock(&t).resize(cols, rows)?;
            Ok(json!({"ok": true}))
        }
        "terminal.attach" => {
            let id = str_of("term_id").to_string();
            let t = s.get_terminal(&id)?;
            let cols = p.get("cols").and_then(Value::as_u64).unwrap_or(80) as usize;
            let rows = p.get("rows").and_then(Value::as_u64).unwrap_or(24) as usize;
            {
                let mut g = lock(&t);
                g.resize(cols, rows)?;
            }
            if let Some(old) = attached.take() {
                if let Ok(old) = s.get_terminal(&old) { lock(&old).unsubscribe(conn_id); }
            }
            {
                let mut g = lock(&t);
                g.subscribe(conn_id, tx.clone());
            }
            *attached = Some(id.clone());
            Ok(json!({"attached": id}))
        }
        "terminal.detach" => {
            if let Some(id) = attached.take() {
                if let Ok(t) = s.get_terminal(&id) {
                    lock(&t).unsubscribe(conn_id);
                }
            }
            Ok(json!({"ok": true}))
        }

        "workspace.list" => Ok(json!(
            s.workspaces.keys().map(|id| s.workspace_json(id)).collect::<Vec<_>>()
        )),
        "workspace.rename" => {
            let id = str_of("workspace_id");
            let label = str_of("label").to_string();
            let ws = s
                .workspaces
                .get_mut(id)
                .ok_or_else(|| format!("no workspace {id}"))?;
            ws.label = if label.is_empty() { None } else { Some(label) };
            s.broadcast_event("workspace.updated", json!({"workspace_id": id}));
            Ok(json!({"ok": true}))
        }

        "tab.create" => {
            let ws = str_of("workspace_id");
            if ws.is_empty() {
                return Err("tab.create needs workspace_id".into());
            }
            let label = p.get("label").and_then(Value::as_str).map(String::from);
            let command = str_of("command");
            s.create_tab(ws, label, command)
        }
        "tab.list" => Ok(json!(
            s.tabs.keys().map(|id| s.tab_json(id)).collect::<Vec<_>>()
        )),
        "tab.close" => {
            s.close_tab(str_of("tab_id"))?;
            Ok(json!({"ok": true}))
        }


        "agent.specs" => {
            let reg = crate::agent::AgentRegistry::load();
            let specs: serde_json::Map<String, Value> = reg
                .specs
                .iter()
                .map(|(n, sp)| (n.clone(), json!({"kind": sp.kind, "args": sp.args})))
                .collect();
            Ok(json!({
                "default": reg.default,
                "specs": Value::Object(specs),
                "config": crate::state::agents_path(),
            }))
        }
        "events.subscribe" => {
            s.event_subs.retain(|out| !Arc::ptr_eq(&out.queued, &tx.queued));
            s.event_subs.push(tx.clone());
            Ok(json!({"subscribed": true}))
        }

        other => Err(format!("unknown method {other}")),
    }
}

/// Commands on an attached connection (control stream, no method envelope).
/// The session lock is held only to resolve the terminal — term locks and
/// PTY writes happen after it drops, so control traffic can't stall API
/// clients.
fn term_command(
    session: &Shared,
    ty: &str,
    msg: &Value,
    conn_id: u64,
    attached: &mut Option<String>,
) -> Result<(), String> {
    let tid = attached.clone().or_else(|| {
        msg.get("term_id").and_then(Value::as_str).map(String::from)
    });
    let tid = tid.ok_or("no attached terminal")?;
    let t = lock(session).get_terminal(&tid)?;
    match ty {
        "input" => {
            lock(&t).user_scrolled = false;
            lock(&t).scroll_to(0);
            term_write(&t, msg.get("text").and_then(Value::as_str).unwrap_or("").as_bytes())?;
        }
        "input_b64" => {
            lock(&t).user_scrolled = false;
            if let Some(b) = msg.get("bytes").and_then(Value::as_str) {
                let bytes = base64::Engine::decode(
                    &base64::engine::general_purpose::STANDARD,
                    b,
                )
                .unwrap_or_default();
                lock(&t).scroll_to(0);
                term_write(&t, &bytes)?;
            }
        }
        "resize" => {
            let cols = msg.get("cols").and_then(Value::as_u64).unwrap_or(80) as usize;
            let rows = msg.get("rows").and_then(Value::as_u64).unwrap_or(24) as usize;
            lock(&t).resize(cols, rows)?;
        }
        "scroll" => {
            {
                let mut g = lock(&t);
                g.user_scrolled = true;
                g.input_seq = g.input_seq.wrapping_add(1);
                g.history_reading = false;
            }
            let col = msg.get("column").and_then(Value::as_u64).unwrap_or(1) as u16;
            let row = msg.get("row").and_then(Value::as_u64).unwrap_or(1) as u16;
            let mods = msg.get("modifiers").and_then(Value::as_u64).unwrap_or(0) as u8;
            let payload = {
                let mut g = lock(&t);
                if let Some(d) = msg.get("delta_px").and_then(Value::as_f64) {
                    let cell = msg.get("cell_px").and_then(Value::as_f64).unwrap_or(14.0);
                    g.scroll_px(d, cell, col, row, mods)
                } else if let Some(d) = msg.get("delta_lines").and_then(Value::as_f64) {
                    g.scroll_lines(d, col, row, mods)
                } else if let Some(o) = msg.get("offset_from_bottom").and_then(Value::as_u64) {
                    g.scroll_to(o as usize);
                    None
                } else {
                    None
                }
            };
            if let Some(bytes) = payload {
                term_write(&t, &bytes)?;
            }
        }
        "detach" => {
            lock(&t).unsubscribe(conn_id);
            *attached = None;
        }
        _ => {}
    }
    Ok(())
}

/// POSIX single-quote escaping for one argv element — safe for arbitrary
/// text (embedded quotes, newlines, utf-8). Commanders pass `args` arrays
/// instead of hand-quoting a shell line inside JSON.
pub(crate) fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "@%_+=:,./-".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::shell_quote;

    #[test]
    fn shell_quote_cases() {
        assert_eq!(shell_quote("--provider"), "--provider");
        assert_eq!(shell_quote("gpt-5.6-luna"), "gpt-5.6-luna");
        assert_eq!(
            shell_quote("분석해줘. \"따옴표\"와 '작은따옴표'\n개행 포함"),
            "'분석해줘. \"따옴표\"와 '\\''작은따옴표'\\''\n개행 포함'"
        );
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("a b"), "'a b'");
    }
}

/// `workspace.create` — git worktree add + a workspace (tab + pane) under
/// the repo's project. Params: repo (path) or project_id, branch, base?,
/// label?, path? — default checkout is ~/.lazed/worktrees/<repo>/<branch-
/// slug>; a taken dir or branch bumps both to a `-N` suffix (response
/// carries the actual branch/checkout).
fn workspace_create(session: &Shared, p: &Value) -> Result<Value, String> {
    let repo = p
        .get("repo")
        .and_then(Value::as_str)
        .map(String::from)
        .or_else(|| {
            p.get("project_id")
                .and_then(Value::as_str)
                .and_then(|id| lock(session).projects.get(id).map(|x| x.repo_root.clone()))
        })
        .ok_or("workspace.create needs repo or project_id")?;
    let branch = p
        .get("branch")
        .and_then(Value::as_str)
        .ok_or("workspace.create needs branch")?
        .to_string();
    let base = p.get("base").and_then(Value::as_str).unwrap_or("");
    let label = p.get("label").and_then(Value::as_str).map(String::from);

    let (repo_root, repo_key) = Session::resolve_repo(&repo);
    let strict = p["strict"] == true;
    let mut resolved_base = base.to_string();
    let mut warnings: Vec<&str> = Vec::new();
    if strict {
        let git = |args: &[&str]| -> Result<String, String> {
            let output = std::process::Command::new("git").args(["-C", &repo_root]).args(args).output().map_err(|e| e.to_string())?;
            if !output.status.success() { return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned()); }
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
        };
        git(&["check-ref-format", "--branch", &branch])?;
        let dirty = !git(&["status", "--porcelain"])?.is_empty();
        if dirty && p["allow_dirty"] != true { return Err("dirty_source: worktrees exclude uncommitted changes; use --allow-dirty only when intended".into()); }
        if dirty { warnings.push("uncommitted source changes are excluded"); }
        resolved_base = git(&["rev-parse", "--verify", "--end-of-options", &format!("{}^{{commit}}", if base.is_empty() { "HEAD" } else { base })])?;
    }
    let repo_name = std::path::Path::new(&repo_root)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "repo".into());
    let slug = branch.replace('/', "-");
    let branch_exists = |b: &str| {
        std::process::Command::new("git")
            .args(["-C", repo_root.as_str(), "show-ref", "--verify", "--quiet"])
            .arg(format!("refs/heads/{b}"))
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    // a taken dir or branch bumps both to `-N`; the response carries the
    // actual names so callers never have to pick a free slug themselves
    let explicit_path = p.get("path").and_then(Value::as_str).map(std::path::PathBuf::from);
    let wt_base = crate::state::worktrees_dir().join(&repo_name);
    let mut branch_name = branch.clone();
    let mut checkout = explicit_path
        .clone()
        .unwrap_or_else(|| wt_base.join(&slug));
    let mut n = 2u32;
    while checkout.exists() || branch_exists(&branch_name) {
        if strict { return Err("worktree branch/path already exists; inspect worktree list, do not blindly retry".into()); }
        if explicit_path.is_some() && checkout.exists() {
            return Err(format!(
                "worktree path already exists: {}",
                checkout.display()
            ));
        }
        branch_name = format!("{branch}-{n}");
        if explicit_path.is_none() {
            checkout = wt_base.join(format!("{slug}-{n}"));
        }
        n += 1;
    }
    if explicit_path.is_none() {
        let _ = std::fs::create_dir_all(&wt_base);
    }

    let mut args = vec!["-C".to_string(), repo_root.clone(), "worktree".into(), "add".into()];
    args.push(checkout.to_string_lossy().to_string());
    args.push("-b".into());
    args.push(branch_name.clone());
    if !resolved_base.is_empty() {
        args.push(resolved_base.clone());
    }
    let out = std::process::Command::new("git")
        .args(&args)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }

    // find (or implicitly create) the project owning this repo
    let mut s = lock(session);
    let project_id = s
        .projects
        .iter()
        .find(|(_, x)| x.repo_key == repo_key)
        .map(|(id, _)| id.clone())
        .or_else(|| {
            s.create_project(&repo_root, None, None)
                .ok()
                .and_then(|v| {
                    v.get("project")
                        .and_then(|x| x.get("project_id"))
                        .and_then(Value::as_str)
                        .map(String::from)
                })
        })
        .ok_or("no project for repo")?;

    let ws = s.create_workspace(
        &project_id,
        &checkout.to_string_lossy(),
        label,
        Some(branch_name.clone()),
        false,
    )?;
    Ok(json!({
        "checkout_path": checkout.to_string_lossy(),
        "branch": branch_name,
        "project_id": project_id,
        "workspace_id": ws.get("workspace").and_then(|w| w.get("workspace_id")),
        "tab_id": ws.get("tab_id"),
        "terminal": ws.get("terminal"),
        "base_commit": if strict { Some(&resolved_base) } else { None },
        "warnings": warnings,
    }))
}

/// ── worktree checkout removal ────────────────────────────────────────────
/// Ported from Orca's removal pipeline (shared/worktree/removal.js + the
/// relay's removeWorktree): every `git worktree remove` refusal mode gets a
/// deterministic outcome — a clean refusal, an automatic retry, or the
/// trash-rename fast path.

/// Run git under one context prefix (`--git-dir=<common>` or `-C <dir>`).
fn git_out(prefix: &[String], args: &[&str]) -> Result<std::process::Output, String> {
    std::process::Command::new("git")
        .args(prefix)
        .args(args)
        .output()
        .map_err(|e| e.to_string())
}

fn git_ok(prefix: &[String], args: &[&str]) -> Result<String, String> {
    let out = git_out(prefix, args)?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    } else {
        Err(stderr_text(&out))
    }
}

fn stderr_text(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).trim().to_string()
}

/// One `git worktree list --porcelain` entry.
struct WtEntry {
    path: String,
    branch: Option<String>,
    head: Option<String>,
    locked: bool,
    lock_reason: Option<String>,
}

fn worktree_entries(prefix: &[String]) -> Result<Vec<WtEntry>, String> {
    let text = git_ok(prefix, &["worktree", "list", "--porcelain"])?;
    Ok(parse_worktree_list(&text))
}

fn parse_worktree_list(text: &str) -> Vec<WtEntry> {
    let mut entries = Vec::new();
    let mut cur: Option<WtEntry> = None;
    for line in text.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            if let Some(e) = cur.take() {
                entries.push(e);
            }
            cur = Some(WtEntry {
                path: p.trim().to_string(),
                branch: None,
                head: None,
                locked: false,
                lock_reason: None,
            });
            continue;
        }
        let Some(e) = cur.as_mut() else { continue };
        if let Some(h) = line.strip_prefix("HEAD ") {
            e.head = Some(h.trim().to_string());
        } else if let Some(b) = line.strip_prefix("branch ") {
            e.branch = Some(b.trim().trim_start_matches("refs/heads/").to_string());
        } else if line == "locked" {
            e.locked = true;
        } else if let Some(r) = line.strip_prefix("locked ") {
            e.locked = true;
            e.lock_reason = Some(r.trim().to_string());
        }
    }
    if let Some(e) = cur {
        entries.push(e);
    }
    entries
}

/// Spelling differences (trailing slash, symlinked parents like macOS
/// /tmp → /private/tmp) still refer to the same checkout.
fn same_checkout_path(a: &str, b: &str) -> bool {
    fn norm(s: &str) -> &str {
        s.trim_end_matches('/')
    }
    if norm(a) == norm(b) {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// A git-locked worktree is an external safety contract — Orca never folds
/// it into the dirty-file force path; the caller must unlock it explicitly.
fn locked_worktree_error(reason: Option<&str>) -> String {
    let suffix = "Run git worktree unlock <worktree-path> from its repository, then retry deletion.";
    match reason.map(str::trim).filter(|r| !r.is_empty()) {
        Some(r) => format!("Worktree is locked by Git. Lock reason: {r}. {suffix}"),
        None => format!("Worktree is locked by Git. {suffix}"),
    }
}

/// `git worktree remove` categorically refuses a worktree containing an
/// initialized submodule — even a fully clean one (validate_no_submodules).
fn is_submodule_removal_refusal(stderr: &str) -> bool {
    stderr
        .to_lowercase()
        .contains("working trees containing submodules cannot be moved or removed")
}

fn is_locked_removal_refusal(stderr: &str) -> bool {
    stderr.contains("cannot remove a locked working tree")
}

/// `git branch -d` refusing because the branch is checked out elsewhere.
fn is_checked_out_branch_error(stderr: &str) -> bool {
    let l = stderr.to_lowercase();
    (l.contains("cannot delete branch")
        && (l.contains("used by worktree") || l.contains("checked out")))
        || (l.contains("branch") && l.contains("is checked out"))
}

/// `git status --porcelain` inside the checkout — the cleanliness proof both
/// the unforced trash path and the submodule-refusal retry need.
fn checkout_status(path: &str) -> Result<String, String> {
    let out = git_out(
        &["-C".to_string(), path.to_string()],
        &["status", "--porcelain", "--untracked-files=all"],
    )?;
    if !out.status.success() {
        return Err(stderr_text(&out));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Sibling of a removed checkout that receives renamed-away worktrees so the
/// slow `rm -rf` runs off the request path.
const WORKTREE_TRASH_DIR: &str = ".lazed-worktree-trash";
static TRASH_SEQ: AtomicUsize = AtomicUsize::new(0);

/// Move the checkout aside into `<parent>/.lazed-worktree-trash/wt-<ms>-<r>`.
/// Instant on the same filesystem, and a directory a process still holds
/// open moves with it instead of blocking deletion.
fn trash_rename(path: &str) -> Option<std::path::PathBuf> {
    let src = std::path::Path::new(path);
    let root = src.parent()?.join(WORKTREE_TRASH_DIR);
    std::fs::create_dir_all(&root).ok()?;
    let md = std::fs::symlink_metadata(&root).ok()?;
    if !md.is_dir() || md.file_type().is_symlink() {
        return None;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let uniq = (nanos as u32)
        ^ std::process::id()
        ^ (TRASH_SEQ.fetch_add(1, Ordering::Relaxed) as u32);
    let dst = root.join(format!("wt-{}-{uniq:08x}", nanos / 1_000_000));
    std::fs::rename(src, &dst).ok()?;
    Some(dst)
}

/// Drop the moved checkout's git registration: `worktree remove` can't find
/// the moved dir (expected) so `prune` clears the admin entry — then the
/// worktree list is re-read to prove the registration is actually gone.
fn deregister_worktree(prefix: &[String], registered_path: &str) -> Result<(), String> {
    if git_ok(prefix, &["worktree", "remove", "--force", registered_path]).is_ok() {
        return Ok(());
    }
    git_ok(prefix, &["worktree", "prune"])?;
    let still = worktree_entries(prefix)?
        .iter()
        .any(|e| same_checkout_path(&e.path, registered_path));
    if still {
        return Err(format!(
            "git still reports a registration for {registered_path} after pruning it"
        ));
    }
    Ok(())
}

/// Serialized background `rm -rf` of trashed checkouts — Orca serializes the
/// same queue so mass removals never storm the filesystem. Failures stay on
/// disk for the boot-time sweep to retry.
fn delete_trash_async(path: std::path::PathBuf) {
    use std::sync::OnceLock;
    static QUEUE: OnceLock<std::sync::mpsc::Sender<std::path::PathBuf>> = OnceLock::new();
    let tx = QUEUE.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<std::path::PathBuf>();
        std::thread::spawn(move || {
            while let Ok(p) = rx.recv() {
                for attempt in 0..3 {
                    match std::fs::remove_dir_all(&p) {
                        Ok(()) => break,
                        Err(e) if attempt == 2 => {
                            eprintln!(
                                "lazed: cannot delete trashed worktree {}: {e}",
                                p.display()
                            );
                        }
                        _ => std::thread::sleep(std::time::Duration::from_millis(500)),
                    }
                }
                // leave no empty trash root behind
                if let Some(root) = p.parent() {
                    let _ = std::fs::remove_dir(root);
                }
            }
        });
        tx
    });
    let _ = tx.send(path);
}

/// Boot-time sweep: delete `.lazed-worktree-trash/wt-*` leftovers a previous
/// daemon never finished removing (crash mid-removal). Scans the managed
/// worktrees root plus each repo dir inside it — the two levels lazed puts
/// checkouts under.
pub fn sweep_worktree_trash() {
    let base = state::worktrees_dir();
    let mut roots = vec![base.clone()];
    if let Ok(rd) = std::fs::read_dir(&base) {
        for e in rd.flatten().take(200) {
            if e.file_name() == WORKTREE_TRASH_DIR {
                continue;
            }
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                roots.push(e.path());
            }
        }
    }
    for root in roots {
        let trash = root.join(WORKTREE_TRASH_DIR);
        if let Ok(rd) = std::fs::read_dir(&trash) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().starts_with("wt-")
                    && e.file_type().is_ok_and(|t| t.is_dir())
                {
                    delete_trash_async(e.path());
                }
            }
        }
        let _ = std::fs::remove_dir(&trash);
    }
}

/// The trash-rename fast path: move the checkout aside, drop its git
/// registration, delete it in the background. Returns None when the path
/// isn't applicable and the caller should run `git worktree remove` instead.
fn try_trash_remove(
    prefix: &[String],
    path: &str,
    registered_path: &str,
    force: bool,
) -> Result<Option<std::path::PathBuf>, String> {
    // Unforced removal must still refuse a dirty checkout — prove clean
    // first so the ordinary path produces its usual refusal instead.
    if !force {
        match checkout_status(path) {
            Ok(s) if s.is_empty() => {}
            _ => return Ok(None),
        }
    }
    let Some(dst) = trash_rename(path) else { return Ok(None) };
    match deregister_worktree(prefix, registered_path) {
        Ok(()) => Ok(Some(dst)),
        Err(e) => match std::fs::rename(&dst, path) {
            // Restored — let the ordinary path report the real error.
            Ok(()) => Ok(None),
            Err(re) => Err(format!(
                "{e}; also failed to restore {path} from trash ({re}) — checkout retained at {}",
                dst.display()
            )),
        },
    }
}

/// Ordinary `git worktree remove` with Orca's two retries: a submodule
/// refusal re-proves cleanliness then retries with --force (git refuses
/// submodule-bearing worktrees even when clean), and a late locked refusal
/// maps to the same actionable error as the upfront check.
fn git_remove_with_retries(prefix: &[String], registered_path: &str, force: bool) -> Result<(), String> {
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(registered_path);
    match git_out(prefix, &args) {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => {
            let err = stderr_text(&o);
            if is_locked_removal_refusal(&err) {
                return Err(locked_worktree_error(None));
            }
            if !force && is_submodule_removal_refusal(&err) {
                return match checkout_status(registered_path) {
                    Ok(s) if s.is_empty() => {
                        match git_out(prefix, &["worktree", "remove", "--force", registered_path]) {
                            Ok(o2) if o2.status.success() => Ok(()),
                            Ok(o2) => Err(format!(
                                "git worktree remove failed: {}",
                                stderr_text(&o2)
                            )),
                            Err(e) => Err(e),
                        }
                    }
                    Ok(s) => Err(format!(
                        "{registered_path} contains modified or untracked files, use --force to delete it\n{s}"
                    )),
                    Err(e) => Err(format!("cannot verify checkout: {e}")),
                };
            }
            Err(format!("git worktree remove failed: {err}"))
        }
        Err(e) => Err(e),
    }
}

/// Orca's deleteBranch default: `git branch -d` the worktree's branch after
/// removal — merged branches disappear, unmerged or checked-out-elsewhere
/// branches are preserved and reported. A transient "checked out" verdict
/// retries once after `worktree prune`. Never fails the removal.
fn delete_worktree_branch(
    prefix: &[String],
    branch: &str,
    head: Option<&str>,
) -> Value {
    if branch.starts_with('-') || branch.contains('\0') {
        return json!({"preserved_branch": {"name": branch}});
    }
    let preserved = |reason: Option<String>| {
        let mut b = json!({"name": branch});
        if let Some(h) = head {
            b["head"] = json!(h);
        }
        if let Some(r) = reason.filter(|r| !r.is_empty()) {
            b["reason"] = json!(r);
        }
        json!({"preserved_branch": b})
    };
    let remove = |p: &[String]| git_ok(p, &["branch", "-d", "--", branch]);
    let first_err = match remove(prefix) {
        Ok(_) => return json!({"deleted_branch": branch}),
        Err(e) => e,
    };
    if !is_checked_out_branch_error(&first_err) {
        return preserved(Some(first_err));
    }
    if git_ok(prefix, &["worktree", "prune"]).is_err() {
        return preserved(None);
    }
    match remove(prefix) {
        Ok(_) => json!({"deleted_branch": branch}),
        Err(e) => preserved(Some(e)),
    }
}

/// Remove the workspace's checkout from disk + git's worktree admin data.
///
///   1. Resolve a git context that reaches the worktree admin data — the
///      checkout's own `--git-common-dir` first (survives the main
///      checkout's deletion), then the recorded repo key, then the checkout
///      path itself (self-removal is legal while the dir lives).
///   2. Refuse a git-locked worktree with an actionable message.
///   3. Prefer the trash-rename fast path: move the checkout into a sibling
///      `.lazed-worktree-trash/` dir, deregister it, delete in background.
///   4. Otherwise `git worktree remove` with the submodule-refusal retry.
///   5. `git branch -d` the worktree's branch unless `keep_branch` — merged
///      branches are deleted, unmerged ones reported preserved.
fn remove_checkout(
    repo_key: Option<&str>,
    path: &str,
    force: bool,
    keep_branch: bool,
    fallback_branch: Option<&str>,
) -> Result<Value, String> {
    let checkout_exists = std::path::Path::new(&path).exists();

    let mut prefixes: Vec<Vec<String>> = Vec::new();
    if checkout_exists {
        if let Ok(common) = git_ok(
            &["-C".to_string(), path.to_string()],
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        ) {
            prefixes.push(vec![format!("--git-dir={common}")]);
        }
    }
    if let Some(key) = repo_key.filter(|k| !k.is_empty()) {
        prefixes.push(vec![format!("--git-dir={key}")]);
    }
    if checkout_exists {
        prefixes.push(vec!["-C".into(), path.into()]);
    }

    // The first context that can actually read the worktree registry.
    let mut ctx: Option<Vec<String>> = None;
    let mut entries: Vec<WtEntry> = Vec::new();
    for p in &prefixes {
        if let Ok(e) = worktree_entries(p) {
            ctx = Some(p.clone());
            entries = e;
            break;
        }
    }
    let entry = entries.iter().find(|e| same_checkout_path(&e.path, path));
    let registered_path = entry.map(|e| e.path.clone());
    if let Some(e) = entry {
        if e.locked {
            return Err(locked_worktree_error(e.lock_reason.as_deref()));
        }
    }
    let branch = entry
        .and_then(|e| e.branch.clone())
        .or_else(|| fallback_branch.map(String::from));
    let head = entry.and_then(|e| e.head.clone());

    if checkout_exists {
        match (&ctx, &registered_path) {
            (Some(prefix), Some(reg)) => {
                if let Some(dst) = try_trash_remove(prefix, path, reg, force)? {
                    delete_trash_async(dst);
                } else {
                    git_remove_with_retries(prefix, reg, force)?;
                }
            }
            (Some(_), None) => {
                // Git never registered this path — a plain directory remains.
                if !force {
                    return Err(format!(
                        "{path} is not registered as a git worktree but its directory remains; use --force to delete it"
                    ));
                }
                match trash_rename(path) {
                    Some(dst) => delete_trash_async(dst),
                    None => std::fs::remove_dir_all(path)
                        .map_err(|e| format!("cannot delete {path}: {e}"))?,
                }
            }
            (None, _) => {
                // No working git context — the checkout can't be verified.
                if !force {
                    return Err(format!(
                        "cannot reach git worktree admin data for {path}; use --force to delete the directory anyway"
                    ));
                }
                match trash_rename(path) {
                    Some(dst) => delete_trash_async(dst),
                    None => std::fs::remove_dir_all(path)
                        .map_err(|e| format!("cannot delete {path}: {e}"))?,
                }
            }
        }
    } else if let Some(prefix) = &ctx {
        // Deleted outside lazed — best-effort deregistration of its stale
        // admin entry; the workspace still closes.
        let _ = deregister_worktree(prefix, registered_path.as_deref().unwrap_or(path));
    }

    let mut result = json!({});
    if !keep_branch {
        if let (Some(prefix), Some(b)) = (&ctx, branch.as_deref()) {
            if let Some(obj) = delete_worktree_branch(prefix, b, head.as_deref()).as_object() {
                for (k, v) in obj {
                    result[k] = v.clone();
                }
            }
        }
    }
    Ok(result)
}

/// `workspace.remove` — close the workspace's tabs/panes, then remove the
/// linked checkout from disk. The main checkout can't be removed this way
/// (project.close handles it). `kill_agents` is the caller's explicit
/// authorization to kill panes that still host an agent process.
fn workspace_remove(session: &Shared, p: &Value) -> Result<Value, String> {
    let mut s = lock(session);
    let wid = p.get("workspace_id").and_then(Value::as_str).unwrap_or("");
    let force = p.get("force").and_then(Value::as_bool).unwrap_or(false);
    let kill_agents = p.get("kill_agents").and_then(Value::as_bool).unwrap_or(false);
    let keep_branch = p.get("keep_branch").and_then(Value::as_bool).unwrap_or(false);
    let ws = s
        .workspaces
        .get(wid)
        .ok_or_else(|| format!("no workspace {wid}"))?;
    if ws.is_main {
        return Err("cannot remove the main workspace — close the project instead".into());
    }
    let path = ws.path.clone();
    let ws_branch = ws.branch.clone();
    if s.removing_workspaces.contains(wid) { return Err("workspace removal in progress".into()); }
    let mut terms = Vec::new();
    // The project's git-common-dir reaches the worktree admin data even
    // after the main checkout itself is deleted — `-C repo_root` would
    // permanently fail in that case. repo_key may hold a plain path for
    // non-repo projects; those never host worktree workspaces anyway.
    let repo_key = s
        .projects
        .get(&ws.project_id)
        .map(|x| x.repo_key.clone())
        .filter(|k| !k.is_empty());
    // A failed git remove must leave panes intact. Refuse live agents
    // unless the caller explicitly authorized killing them — a settled
    // screen is not proof that it is safe to kill the process.
    for tab in &ws.tabs {
        if let Some(tab) = s.tabs.get(tab) {
            for tid in &tab.panes {
                if let Ok(term) = s.get_terminal(tid) {
                    let mut term = lock(&term);
                    term.detect_agent();
                    if !kill_agents && term.agent_kind.is_some() {
                        return Err("workspace_has_agent: stop the agent explicitly before removal, or pass kill_agents/--kill-agents".into());
                    }
                    drop(term);
                    terms.push(s.get_terminal(tid)?);
                }
            }
        }
    }
    let controls: Vec<_> = terms.iter().map(|t| lock(t).agent_control.clone()).collect();
    let _guards: Vec<_> = controls.iter().map(|c| c.try_lock().map_err(|_| "agent_control_busy"))
        .collect::<Result<_, _>>()?;
    let writers: Vec<_> = terms.iter().map(|t| lock(t).writer()).collect();
    let _writers: Vec<_> = writers.iter().map(|w| w.try_lock().map_err(|_| "terminal_input_busy"))
        .collect::<Result<_, _>>()?;
    for t in &terms {
        let mut g = lock(t);
        g.detect_agent();
        if !kill_agents && g.agent_kind.is_some() { return Err("workspace_has_agent".into()); }
    }
    s.removing_workspaces.insert(wid.to_owned());
    for t in &terms { lock(t).maintenance = true; }
    drop(s);
    // A failed removal must leave panes intact — the workspace only closes
    // after the checkout (and its git registration) is really gone.
    let removal = remove_checkout(
        repo_key.as_deref(),
        &path,
        force,
        keep_branch,
        ws_branch.as_deref(),
    );
    let mut s = lock(session);
    s.removing_workspaces.remove(wid);
    for t in &terms { lock(t).maintenance = false; }
    let mut result = removal?;
    if s.workspaces.contains_key(wid) { s.close_workspace(wid)?; }
    result["ok"] = json!(true);
    result["removed"] = json!(path);
    Ok(result)
}

fn worktree_list(p: &Value) -> Result<Value, String> {
    let out = std::process::Command::new("git")
        .args(["-C", crate::control::str_of(p, "repo"), "worktree", "list", "--porcelain"])
        .output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("git worktree list failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(json!({"text": String::from_utf8_lossy(&out.stdout)}))
}

#[cfg(test)]
mod reliability_tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn reattach_removes_old_and_duplicate_subscriptions() {
        let mut session = Session::new();
        let mut readers = Vec::new();
        for id in ["t1", "t2"] {
            let (term, reader) = PtyTerm::spawn(id, "/tmp", "", None, "plain", 80, 24, "w1").unwrap();
            readers.push(reader);
            session.terminals.insert(id.into(), Arc::new(Mutex::new(term)));
        }
        let (socket, _peer) = UnixStream::pair().unwrap();
        let (tx, rx) = sync_channel(128);
        let sender = ClientSender { tx, queued: Arc::new(AtomicUsize::new(0)), socket: Arc::new(socket) };
        let mut attached = None;
        for id in ["t1", "t2", "t2"] {
            dispatch(&mut session, "terminal.attach", &json!({"term_id": id}), 1, &sender, &mut attached, &mut Vec::new()).unwrap();
        }
        while rx.try_recv().is_ok() {}
        lock(&session.terminals["t1"]).emit_frame(true);
        assert!(rx.try_recv().is_err());
        lock(&session.terminals["t2"]).emit_frame(true);
        assert!(rx.try_recv().is_ok());
        assert!(rx.try_recv().is_err());
        for term in session.terminals.values() { lock(term).kill(); }
    }

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

    #[test]
    fn failed_git_list_is_an_error() {
        assert!(worktree_list(&json!({"repo": "/nonexistent-lazed-test-repo"})).unwrap_err().contains("git worktree list failed"));
    }
}
