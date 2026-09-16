//! Unix socket server: NDJSON request/response + pushed events + attach
//! streams. One connection = one client channel (mpsc outbox → writer).
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};

use crate::session::Session;
use crate::state;
use crate::term::{lock, PtyTerm};

type Shared = Arc<Mutex<Session>>;

pub fn run() -> std::io::Result<()> {
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
            lock(&s).persist();
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
    let (tx, rx) = channel::<Value>();
    // writer thread: every outbox value becomes one JSON line on the socket
    std::thread::spawn(move || {
        let mut w = writer_stream;
        for msg in rx {
            let mut line = serde_json::to_string(&msg).unwrap_or_default();
            line.push('\n');
            if w.write_all(line.as_bytes()).is_err() {
                break;
            }
            let _ = w.flush();
        }
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
            let res = if m == "agent.wait" {
                agent_wait(&session, &params)
            } else {
                let mut jobs = Vec::new();
                let r = {
                    let mut s = lock(&session);
                    dispatch(&mut s, m, &params, conn_id, &tx, &mut attached, &mut jobs)
                };
                // PTY writes after the session lock drops — a child that
                // stalls on input must not freeze other clients
                for (t, bytes) in jobs {
                    term_write(&t, &bytes);
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
    if let Some(tid) = attached {
        if let Ok(t) = lock(&session).get_terminal(&tid) {
            lock(&t).unsubscribe(conn_id);
        }
    }
}

/// Write bytes to a terminal's PTY input. The term lock is held only long
/// enough to clone the writer handle — a child that stops draining input
/// stalls this write but never the renderer or the session.
fn term_write(t: &Arc<Mutex<PtyTerm>>, bytes: &[u8]) {
    let w = lock(t).writer();
    let mut w = lock(&w);
    let _ = w.write_all(bytes);
    let _ = w.flush();
}

fn dispatch(
    s: &mut Session,
    method: &str,
    p: &Value,
    conn_id: u64,
    tx: &std::sync::mpsc::Sender<Value>,
    attached: &mut Option<String>,
    jobs: &mut Vec<(Arc<Mutex<PtyTerm>>, Vec<u8>)>,
) -> Result<Value, String> {
    let str_of = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or("");
    match method {
        "session.status" => Ok(json!({
            "running": true,
            "version": env!("CARGO_PKG_VERSION"),
            "terms": s.terminals.len(),
            "projects": s.projects.len(),
        })),
        "session.snapshot" => Ok(s.snapshot()),
        "session.persist" => {
            s.persist();
            Ok(json!({"ok": true}))
        }
        "server.stop" => {
            s.persist();
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

        "terminal.create" => {
            let project = str_of("project_id");
            let cwd = str_of("cwd").to_string();
            let command = str_of("command").to_string();
            let label = p.get("label").and_then(Value::as_str).map(String::from);
            let cwd = if cwd.is_empty() {
                s.projects
                    .get(project)
                    .map(|x| x.repo_root.clone())
                    .unwrap_or_else(|| ".".into())
            } else {
                cwd
            };
            let t = s.create_terminal(
                if project.is_empty() { None } else { Some(project) },
                &cwd,
                &command,
                label,
                "plain",
                80,
                24,
            )?;
            let tid = lock(&t).id.clone();
            Ok(s.terminal_json(&tid))
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
            lock(&t).resize(cols, rows);
            Ok(json!({"ok": true}))
        }
        "terminal.attach" => {
            let id = str_of("term_id").to_string();
            let t = s.get_terminal(&id)?;
            let cols = p.get("cols").and_then(Value::as_u64).unwrap_or(80) as usize;
            let rows = p.get("rows").and_then(Value::as_u64).unwrap_or(24) as usize;
            {
                let mut g = lock(&t);
                g.resize(cols, rows);
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

        "worktree.create" => worktree_create(s, p),
        "worktree.list" => {
            let repo = str_of("repo");
            let out = std::process::Command::new("git")
                .args(["-C", repo, "worktree", "list", "--porcelain"])
                .output()
                .map_err(|e| e.to_string())?;
            Ok(json!({"text": String::from_utf8_lossy(&out.stdout)}))
        }
        "worktree.remove" => worktree_remove(s, p),

        "agent.start" => {
            let t = s.get_terminal(str_of("term_id"))?;
            let (kind, mut args) = crate::agent::AgentRegistry::load()
                .resolve(str_of("spec"), str_of("kind"))?;
            if let Some(a) = p.get("args") {
                for v in a
                    .as_array()
                    .ok_or("agent.start args must be an array of strings")?
                {
                    args.push(
                        v.as_str()
                            .ok_or("agent.start args must be an array of strings")?
                            .to_string(),
                    );
                }
            }
            let mut parts = vec![agent_command(&kind).to_string()];
            for a in &args {
                parts.push(shell_quote(a));
            }
            let cmd = parts.join(" ");
            let tid;
            {
                let mut g = lock(&t);
                g.agent_kind = Some(kind.clone());
                g.agent_status = "idle".into();
                tid = g.id.clone();
            }
            jobs.push((t, format!("{cmd}\n").into_bytes()));
            // the status set above isn't a detect_agent transition — push it
            // so subscribers (badges, inbox) update without waiting a tick
            s.broadcast_event(
                "agent.status",
                json!({"term_id": tid, "agent": kind, "agent_status": "idle"}),
            );
            Ok(json!({"ok": true, "kind": kind, "command": cmd}))
        }
        "agent.prompt" => {
            let t = s.get_terminal(str_of("term_id"))?;
            let text = str_of("text");
            jobs.push((t, format!("{text}\r").into_bytes()));
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
        "agent.get" => {
            let t = s.get_terminal(str_of("term_id"))?;
            let g = lock(&t);
            let fg = g
                .fg_process()
                .map(|(comm, args)| json!({"comm": comm, "args": args}));
            Ok(json!({
                "term_id": g.id,
                "agent": g.agent_kind,
                "agent_status": g.agent_status,
                "fg": fg,
            }))
        }
        "events.subscribe" => {
            s.event_subs.push(tx.clone());
            Ok(json!({"subscribed": true}))
        }

        other => Err(format!("unknown method {other}")),
    }
}

/// `agent.wait` — poll a terminal's status until it matches `until`.
/// "done" satisfies a wait for "idle": a finished agent is sitting idle.
/// Called without the session lock — the terminal Arc is extracted first,
/// so a parked wait never blocks other requests.
fn agent_wait(session: &Shared, p: &Value) -> Result<Value, String> {
    let t = {
        let s = lock(session);
        s.get_terminal(p.get("term_id").and_then(Value::as_str).unwrap_or(""))?
    };
    let until: Vec<String> = p
        .get("until")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect())
        .unwrap_or_else(|| vec!["done".into(), "idle".into(), "blocked".into()]);
    let timeout = p.get("timeout_ms").and_then(Value::as_u64).unwrap_or(0);
    let start = std::time::Instant::now();
    loop {
        {
            let g = lock(&t);
            let hit = until
                .iter()
                .any(|u| *u == g.agent_status || (*u == "idle" && g.agent_status == "done"));
            if hit {
                return Ok(json!({"status": g.agent_status}));
            }
        }
        if timeout > 0 && start.elapsed().as_millis() as u64 > timeout {
            return Err("agent_wait_timeout".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
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
            term_write(&t, msg.get("text").and_then(Value::as_str).unwrap_or("").as_bytes());
        }
        "input_b64" => {
            if let Some(b) = msg.get("bytes").and_then(Value::as_str) {
                let bytes = base64::Engine::decode(
                    &base64::engine::general_purpose::STANDARD,
                    b,
                )
                .unwrap_or_default();
                term_write(&t, &bytes);
            }
        }
        "resize" => {
            let cols = msg.get("cols").and_then(Value::as_u64).unwrap_or(80) as usize;
            let rows = msg.get("rows").and_then(Value::as_u64).unwrap_or(24) as usize;
            lock(&t).resize(cols, rows);
        }
        "scroll" => {
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
                term_write(&t, &bytes);
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
fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "@%_+=:,./-".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

fn agent_command(kind: &str) -> &str {
    match kind {
        "claude" => "claude",
        "codex" => "codex",
        "antigravity" => "antigravity",
        "devin" => "devin",
        "gemini" => "gemini",
        "pi" => "pi",
        other => other,
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

/// `worktree.create` — git worktree add + a worktree terminal under the
/// repo's project. Params: repo (path) or project_id, branch, base?, label?,
/// path? — default checkout is ~/.lazed/worktrees/<repo>/<branch-slug>;
/// a taken dir or branch bumps both to a `-N` suffix (response carries the
/// actual branch/checkout).
fn worktree_create(s: &mut Session, p: &Value) -> Result<Value, String> {
    let repo = p
        .get("repo")
        .and_then(Value::as_str)
        .map(String::from)
        .or_else(|| {
            p.get("project_id")
                .and_then(Value::as_str)
                .and_then(|id| s.projects.get(id).map(|x| x.repo_root.clone()))
        })
        .ok_or("worktree.create needs repo or project_id")?;
    let branch = p
        .get("branch")
        .and_then(Value::as_str)
        .ok_or("worktree.create needs branch")?
        .to_string();
    let base = p.get("base").and_then(Value::as_str).unwrap_or("");
    let label = p.get("label").and_then(Value::as_str).map(String::from);

    let (repo_root, repo_key) = Session::resolve_repo(&repo);
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
    if !base.is_empty() {
        args.push(base.to_string());
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
    let project_id = s
        .projects
        .iter()
        .find(|(_, x)| x.repo_key == repo_key)
        .map(|(id, _)| id.clone())
        .or_else(|| {
            s.create_project(&repo_root, None, None)
                .ok()
                .and_then(|v| v.get("project").and_then(|x| x.get("project_id")).and_then(Value::as_str).map(String::from))
        })
        .ok_or("no project for repo")?;

    let t = s.create_terminal(
        Some(&project_id),
        &checkout.to_string_lossy(),
        "",
        label,
        "worktree",
        80,
        24,
    )?;
    let tid = lock(&t).id.clone();
    Ok(json!({
        "checkout_path": checkout.to_string_lossy(),
        "branch": branch_name,
        "project_id": project_id,
        "terminal": s.terminal_json(&tid),
    }))
}

fn worktree_remove(s: &mut Session, p: &Value) -> Result<Value, String> {
    let tid = p.get("term_id").and_then(Value::as_str).unwrap_or("");
    let force = p.get("force").and_then(Value::as_bool).unwrap_or(false);
    let t = s.get_terminal(tid)?;
    let path = { lock(&t).cwd.clone() };
    // close the terminal first so no PTY is left in the dir
    s.close_terminal(tid)?;
    let mut args = vec!["worktree".into(), "remove".into(), path.clone()];
    if force {
        args.push("--force".into());
    }
    // find a repo that owns it — use the worktree itself (git resolves)
    let out = std::process::Command::new("git")
        .args(["-C", &path])
        .args(&args)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "git worktree remove failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(json!({"ok": true, "removed": path}))
}
