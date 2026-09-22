//! Unix socket server: NDJSON request/response + pushed events. One
//! connection = one client channel (mpsc outbox → writer).
//!
//! Path C: this daemon serves the organization layer (groups, projects,
//! worktree lifecycle, tasks, inbox) and re-broadcasts herdr's events.
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
        } else if m == "workspace.create" {
            workspace_create(&session, &params)
        } else if m == "workspace.remove" {
            workspace_remove(&session, &params)
        } else if m == "workspace.rename" {
            workspace_rename(&session, &params)
        } else if m == "worktree.list" {
            worktree_list(&params)
        } else if m.starts_with("herdr.") {
            herdr_call(m, &params)
        } else if m.starts_with("task.") {
            crate::tasks::handle(&session, m, &params).map(crate::tasks::public)
        } else if m.starts_with("inbox.") {
            let mut s = lock(&session);
            crate::inbox::handle(&mut s, m, &params)
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
                "capabilities": ["herdr.overlay.v1", "task.v1", "inbox.v1", "server.restart.v1", "server.stop_if_empty.v1"],
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
            // herdr keeps the panes alive; "empty" here means no removal
            // pipeline is mid-flight. Exit while holding the lock so nothing
            // slips between the check and the exit.
            if !s.removing_workspaces.is_empty() {
                return Err("daemon_busy: worktree removal in progress".into());
            }
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
            (id, cwd, root, key, branch)
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
    let s = lock(session);
    Ok(json!({
        "project": s.project_json(&project_id),
        "workspace": s.main_workspace(&project_id).map(|w| s.workspace_json(&w.id)),
        "opened": opened,
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

/// `workspace.create` — git worktree add + open the checkout as a herdr
/// workspace under the repo's project. Params: repo (path) or project_id,
/// branch, base?, label?, path? — default checkout is
/// ~/.lazed/worktrees/<repo>/<branch-slug>; a taken dir or branch bumps
/// both to a `-N` suffix (response carries the actual branch/checkout).
/// The result carries the herdr workspace/tab/pane ids — agents start
/// with `herdr agent start … --pane <pane_id>`.
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

    // open the checkout in herdr — a failure here leaves the git worktree
    // in place (the caller can open it later); never silently delete it
    let checkout_str = checkout.to_string_lossy().to_string();
    let ws_label = label.clone().unwrap_or_else(|| branch_name.clone());
    let (ws, tab, pane) = herdr::workspace_create(&checkout_str, Some(&ws_label))
        .map_err(|e| format!("worktree added at {checkout_str} but herdr could not open it: {e}"))?;

    let mut s = lock(session);
    let project_id = s.ensure_project(&repo_root, &repo_key, None, None)?;
    s.register_workspace(&ws, &project_id, &checkout_str, label, Some(branch_name.clone()), false)?;
    Ok(json!({
        "checkout_path": checkout_str,
        "branch": branch_name,
        "project_id": project_id,
        "workspace_id": ws,
        "tab_id": tab,
        "pane_id": pane,
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

/// Canonical spelling of a checkout path — resolves symlinked parents even
/// after the checkout dir itself is already gone (macOS /var → /private/var),
/// so git's recorded path still matches the workspace's stored path.
fn canonical_spelling(path: &str) -> String {
    if let Ok(p) = std::fs::canonicalize(path) {
        return p.to_string_lossy().into_owned();
    }
    let p = std::path::Path::new(path);
    match (p.parent(), p.file_name()) {
        (Some(parent), Some(name)) => std::fs::canonicalize(parent)
            .map(|c| c.join(name).to_string_lossy().into_owned())
            .unwrap_or_else(|_| path.to_string()),
        _ => path.to_string(),
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
    let checkout_exists = std::path::Path::new(path).exists();

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
    let canon = canonical_spelling(path);
    let entry = entries.iter().find(|e| same_checkout_path(&e.path, &canon));
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
    let wid = p.get("workspace_id").and_then(Value::as_str).unwrap_or("");
    let force = p.get("force").and_then(Value::as_bool).unwrap_or(false);
    let kill_agents = p.get("kill_agents").and_then(Value::as_bool).unwrap_or(false);
    let keep_branch = p.get("keep_branch").and_then(Value::as_bool).unwrap_or(false);
    let (path, ws_branch, repo_key) = {
        let mut s = lock(session);
        let ws = s
            .workspaces
            .get(wid)
            .ok_or_else(|| format!("no workspace {wid}"))?;
        if ws.is_main {
            return Err("cannot remove the main workspace — close the project instead".into());
        }
        if s.removing_workspaces.contains(wid) { return Err("workspace removal in progress".into()); }
        // The project's git-common-dir reaches the worktree admin data even
        // after the main checkout itself is deleted — `-C repo_root` would
        // permanently fail in that case. repo_key may hold a plain path for
        // non-repo projects; those never host worktree workspaces anyway.
        let repo_key = s
            .projects
            .get(&ws.project_id)
            .map(|x| x.repo_key.clone())
            .filter(|k| !k.is_empty());
        let out = (ws.path.clone(), ws.branch.clone(), repo_key);
        s.removing_workspaces.insert(wid.to_owned());
        out
    };
    let finish = |session: &Shared| { lock(session).removing_workspaces.remove(wid); };
    // Refuse live agents unless the caller explicitly authorized killing
    // them — herdr's agent list is the truth about who lives in the panes.
    // herdr down → nothing to kill, panes are already gone with it.
    match herdr::snapshot() {
        Ok(snap) => {
            let agents: Vec<String> = snap
                .get("agents")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter(|x| x.get("workspace_id").and_then(Value::as_str) == Some(wid))
                        .filter(|x| x.get("agent").is_some_and(|k| !k.is_null()))
                        .filter_map(|x| x.get("pane_id").and_then(Value::as_str).map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            if !agents.is_empty() && !kill_agents {
                finish(session);
                return Err(format!(
                    "workspace_has_agent: panes {} host agents; stop them explicitly before removal, or pass kill_agents/--kill-agents",
                    agents.join(", ")
                ));
            }
        }
        Err(e) if e.starts_with("herdr_not_running") || e.starts_with("herdr_not_installed") => {}
        Err(e) => {
            finish(session);
            return Err(e);
        }
    }
    // A failed removal must leave the herdr workspace intact — it only
    // closes after the checkout (and its git registration) is really gone.
    let removal = remove_checkout(
        repo_key.as_deref(),
        &path,
        force,
        keep_branch,
        ws_branch.as_deref(),
    );
    finish(session);
    let mut result = removal?;
    let herdr_close = herdr::call("workspace.close", json!({"workspace_id": wid}));
    let mut s = lock(session);
    if s.workspaces.contains_key(wid) { s.close_workspace(wid)?; }
    result["ok"] = json!(true);
    result["removed"] = json!(path);
    if let Err(e) = herdr_close {
        result["herdr_close_error"] = json!(e);
    }
    Ok(result)
}

fn worktree_list(p: &Value) -> Result<Value, String> {
    let out = std::process::Command::new("git")
        .args(["-C", p.get("repo").and_then(Value::as_str).unwrap_or(""), "worktree", "list", "--porcelain"])
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

#[cfg(test)]
mod removal_tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn tdir(tag: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let d = std::env::temp_dir().join(format!("lazed-rm-{tag}-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn sh_git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A repo with one commit; returns (repo, git-common-dir).
    fn mk_repo(tag: &str) -> (PathBuf, String) {
        let repo = tdir(tag).join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        sh_git(&repo, &["init", "-q"]);
        sh_git(
            &repo,
            &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "init", "--allow-empty"],
        );
        let common = sh_git(&repo, &["rev-parse", "--path-format=absolute", "--git-common-dir"]);
        (repo, common)
    }

    fn add_worktree(repo: &Path, name: &str) -> PathBuf {
        let wt = repo.parent().unwrap().join(name);
        sh_git(repo, &["worktree", "add", "-q", "-b", name, wt.to_str().unwrap()]);
        wt
    }

    fn registered(repo: &Path, wt: &Path) -> bool {
        sh_git(repo, &["worktree", "list", "--porcelain"])
            .contains(&format!("worktree {}", wt.canonicalize().unwrap_or_else(|_| wt.into()).display()))
    }

    #[test]
    fn parses_porcelain_branch_locked_detached() {
        let text = "worktree /repo\nHEAD aaa\nbranch refs/heads/main\n\nworktree /wt\nHEAD bbb\ndetached\n\nworktree /lk\nHEAD ccc\nbranch refs/heads/lk\nlocked reasons here\n";
        let e = parse_worktree_list(text);
        assert_eq!(e.len(), 3);
        assert_eq!(e[0].branch.as_deref(), Some("main"));
        assert!(e[1].branch.is_none());
        assert!(e[2].locked && e[2].lock_reason.as_deref() == Some("reasons here"));
    }

    #[test]
    fn refusal_matchers() {
        assert!(is_submodule_removal_refusal("fatal: working trees containing submodules cannot be moved or removed"));
        assert!(!is_submodule_removal_refusal("fatal: 'x' contains modified or untracked files"));
        assert!(is_locked_removal_refusal("fatal: cannot remove a locked working tree;"));
        assert!(is_checked_out_branch_error("error: cannot delete branch 'x' used by worktree at /y"));
        assert!(is_checked_out_branch_error("fatal: branch 'x' is checked out at /y"));
        assert!(!is_checked_out_branch_error("error: the branch 'x' is not fully merged"));
    }

    #[test]
    fn removes_clean_worktree_and_deletes_merged_branch() {
        let (repo, common) = mk_repo("clean");
        let wt = add_worktree(&repo, "feat");
        let v = remove_checkout(Some(common.as_str()), wt.to_str().unwrap(), false, false, None).unwrap();
        assert!(!wt.exists());
        assert_eq!(v["deleted_branch"], "feat");
        assert!(!registered(&repo, &wt));
    }

    #[test]
    fn dirty_worktree_refuses_then_force_removes() {
        let (repo, common) = mk_repo("dirty");
        let wt = add_worktree(&repo, "dirt");
        std::fs::write(wt.join("untracked.txt"), "x").unwrap();
        let err = remove_checkout(Some(common.as_str()), wt.to_str().unwrap(), false, false, None)
            .unwrap_err();
        assert!(err.contains("modified or untracked"), "{err}");
        assert!(wt.exists());
        remove_checkout(Some(common.as_str()), wt.to_str().unwrap(), true, false, None).unwrap();
        assert!(!wt.exists());
        assert!(!registered(&repo, &wt));
    }

    #[test]
    fn clean_submodule_worktree_is_removed() {
        let (repo, common) = mk_repo("subm");
        let wt = add_worktree(&repo, "wt-sub");
        sh_git(&wt, &["-c", "protocol.file.allow=always", "submodule", "add", "-q", repo.to_str().unwrap(), "sub1"]);
        sh_git(&wt, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qam", "sub"]);
        assert!(checkout_status(wt.to_str().unwrap()).unwrap().is_empty());
        // plain git would refuse — the daemon must force past it after a
        // clean proof (or bypass it entirely via the trash path).
        let plain = git_out(
            &["-C".into(), repo.to_string_lossy().into_owned()],
            &["worktree", "remove", wt.to_str().unwrap()],
        )
        .unwrap();
        assert!(is_submodule_removal_refusal(&stderr_text(&plain)));
        remove_checkout(Some(common.as_str()), wt.to_str().unwrap(), false, false, None).unwrap();
        assert!(!wt.exists());
        assert!(!registered(&repo, &wt));
    }

    #[test]
    fn locked_worktree_is_refused_even_with_force() {
        let (repo, common) = mk_repo("lock");
        let wt = add_worktree(&repo, "lk");
        sh_git(&repo, &["worktree", "lock", "--reason", "busy", wt.to_str().unwrap()]);
        let err = remove_checkout(Some(common.as_str()), wt.to_str().unwrap(), true, false, None).unwrap_err();
        assert!(err.contains("locked by Git") && err.contains("busy"), "{err}");
        assert!(wt.exists());
        sh_git(&repo, &["worktree", "unlock", wt.to_str().unwrap()]);
        remove_checkout(Some(common.as_str()), wt.to_str().unwrap(), false, false, None).unwrap();
        assert!(!wt.exists());
    }

    #[test]
    fn unmerged_branch_is_preserved() {
        let (repo, common) = mk_repo("unmerged");
        let wt = add_worktree(&repo, "wip");
        std::fs::write(wt.join("new.txt"), "work").unwrap();
        sh_git(&wt, &["add", "new.txt"]);
        sh_git(&wt, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "wip"]);
        let v = remove_checkout(Some(common.as_str()), wt.to_str().unwrap(), false, false, None).unwrap();
        assert!(!wt.exists());
        assert_eq!(v["preserved_branch"]["name"], "wip");
        sh_git(&repo, &["show-ref", "--verify", "--quiet", "refs/heads/wip"]);
    }

    #[test]
    fn externally_deleted_checkout_is_deregistered() {
        let (repo, common) = mk_repo("gone");
        let wt = add_worktree(&repo, "gone");
        std::fs::remove_dir_all(&wt).unwrap();
        let v = remove_checkout(Some(common.as_str()), wt.to_str().unwrap(), false, false, None).unwrap();
        assert_eq!(v["deleted_branch"], "gone");
        assert!(!registered(&repo, &wt));
    }

    #[test]
    fn unregistered_directory_requires_force() {
        let (_repo, common) = mk_repo("unreg");
        let dir = _repo.parent().unwrap().join("plain-dir");
        std::fs::create_dir_all(&dir).unwrap();
        let err = remove_checkout(Some(common.as_str()), dir.to_str().unwrap(), false, false, None).unwrap_err();
        assert!(err.contains("--force"), "{err}");
        assert!(dir.exists());
        remove_checkout(Some(common.as_str()), dir.to_str().unwrap(), true, false, None).unwrap();
        assert!(!dir.exists());
    }

    #[test]
    fn keep_branch_retains_the_branch() {
        let (repo, common) = mk_repo("keep");
        let wt = add_worktree(&repo, "keepme");
        remove_checkout(Some(common.as_str()), wt.to_str().unwrap(), false, true, None).unwrap();
        assert!(!wt.exists());
        sh_git(&repo, &["show-ref", "--verify", "--quiet", "refs/heads/keepme"]);
    }

    #[test]
    fn stale_trash_is_swept() {
        let base = tdir("sweep");
        let prev = std::env::var("LAZED_WORKTREE_DIR").ok();
        std::env::set_var("LAZED_WORKTREE_DIR", &base);
        let trash = base.join("repo").join(WORKTREE_TRASH_DIR);
        let stale = trash.join("wt-1-deadbeef");
        std::fs::create_dir_all(&stale).unwrap();
        sweep_worktree_trash();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while stale.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        match prev {
            Some(v) => std::env::set_var("LAZED_WORKTREE_DIR", v),
            None => std::env::remove_var("LAZED_WORKTREE_DIR"),
        }
        assert!(!stale.exists());
    }
}
