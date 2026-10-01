mod automation;
mod automation_gate;
mod env;
mod fs;
mod fs_watch;
mod git;
#[path = "../../shared/repo.rs"]
mod repo;
mod herdr;
mod integrations;
mod lazed;
mod process;
mod state_file;

use std::collections::HashMap;
use std::io::BufRead;
use std::sync::{Arc, Mutex};

use serde_json::value::RawValue;
use serde_json::{json, Value};
use tauri::ipc::Channel;
use tauri::{Emitter, Manager, State};

struct AppState {
    watches: Arc<fs_watch::Watches>,
    /// live herdr terminal streams, by pane id
    control: Arc<Mutex<HashMap<String, herdr::ControlHandle>>>,
    events_running: Mutex<bool>,
    /// The live webview's event channel. A reload re-invokes
    /// subscribe_events with a fresh Channel — the stream thread must
    /// send to the newest one.
    events_channel: Arc<Mutex<Option<Channel<Box<RawValue>>>>>,
}

/// Boot both daemons (rule 4: the app auto-detect-launches each; they
/// never spawn one another). herdr failing to start is degraded mode, not
/// a boot failure — the organization layer still renders.
#[tauri::command]
async fn bootstrap(state: State<'_, AppState>) -> Result<Value, String> {
    let control = state.control.clone();
    tauri::async_runtime::spawn_blocking(move || {
    let herdr_error = herdr::ensure_server().err();
    if let Some(e) = &herdr_error {
        eprintln!("[lazed] herdr unavailable: {e}");
    }
    lazed::ensure_server()?;
    let snap = lazed::api_call("session.snapshot", json!({}))?;
    // drop stale control streams whose pane disappeared
    let live: Vec<String> = snap
        .pointer("/panes")
        .and_then(Value::as_array)
        .map(|ps| {
            ps.iter()
                .filter_map(|p| p.get("pane_id").and_then(Value::as_str).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if let Ok(mut map) = control.lock() {
        let stale: Vec<String> = map
            .keys()
            .filter(|id| !live.contains(id))
            .cloned()
            .collect();
        for id in stale {
            if let Some(h) = map.remove(&id) {
                h.close();
            }
        }
    }
    Ok(json!({"snapshot": snap, "herdr_error": herdr_error}))

    }).await.map_err(|e| e.to_string())?
}

/// herdr adapter + server health, as the daemon sees it.
#[tauri::command]
async fn herdr_status() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    lazed::api_call("herdr.status", json!({}))

    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn session_snapshot() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    lazed::api_call("session.snapshot", json!({}))

    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn session_status() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    lazed::api_call("session.status", json!({}))

    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn server_restart(only_if_empty: Option<bool>) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || lazed::restart_server(only_if_empty.unwrap_or(false)))
        .await
        .map_err(|e| e.to_string())?
}

// ── herdr terminal streams (one `terminal session control` child per pane) ──

#[tauri::command]
fn pane_attach(
    pane_id: String,
    cols: u32,
    rows: u32,
    on_frame: Channel<Box<RawValue>>,
    state: State<AppState>,
) -> Result<(), String> {
    detach_pane_internal(&state, &pane_id);

    let (handle, mut reader) = herdr::open_control_stream(&pane_id, cols, rows)?;
    state
        .control
        .lock()
        .map_err(|e| e.to_string())?
        .insert(pane_id.clone(), handle);

    let tid = pane_id.clone();
    std::thread::spawn(move || {
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    // Frames are forwarded as raw JSON — no Value DOM
                    // round-trip per line on the hot path.
                    let raw = match serde_json::from_str::<Box<RawValue>>(trimmed) {
                        Ok(v) => v,
                        Err(_) => raw_json(&json!({"type": "log", "text": trimmed})),
                    };
                    if on_frame.send(raw).is_err() {
                        break;
                    }
                }
            }
        }
        let _ = on_frame.send(raw_json(&json!({"type": "terminal.closed"})));
        eprintln!("[lazed] herdr stream ended for {tid}");
    });
    Ok(())
}

/// Serialize a Value into a channel payload — json! output is always
/// valid JSON so validation cannot fail.
pub(crate) fn raw_json(v: &Value) -> Box<RawValue> {
    RawValue::from_string(v.to_string()).expect("serialized json! is valid")
}

fn send_control(state: &State<AppState>, pane_id: &str, msg: &Value) -> Result<(), String> {
    let mut map = state.control.lock().map_err(|e| e.to_string())?;
    let handle = map
        .get_mut(pane_id)
        .ok_or_else(|| format!("no control stream for {pane_id}"))?;
    handle.send(msg)
}

#[tauri::command]
fn pane_input(pane_id: String, text: String, state: State<AppState>) -> Result<(), String> {
    send_control(&state, &pane_id, &herdr::cmd_input_text(&text))
}

#[tauri::command]
fn pane_resize(pane_id: String, cols: u32, rows: u32, state: State<AppState>) -> Result<(), String> {
    send_control(&state, &pane_id, &herdr::cmd_resize(cols, rows))
}

/// Whole-line scroll of herdr's scrollback — the webview converts wheel
/// pixels to lines; positive = toward history.
#[tauri::command]
fn pane_scroll(pane_id: String, lines: i64, state: State<AppState>) -> Result<(), String> {
    match herdr::cmd_scroll(lines) {
        Some(msg) => send_control(&state, &pane_id, &msg),
        None => Ok(()),
    }
}

#[tauri::command]
fn pane_detach(pane_id: String, state: State<AppState>) -> Result<(), String> {
    detach_pane_internal(&state, &pane_id);
    Ok(())
}

fn detach_pane_internal(state: &State<AppState>, pane_id: &str) {
    if let Ok(mut map) = state.control.lock() {
        if let Some(handle) = map.remove(pane_id) {
            handle.close();
        }
    }
}

// ── model methods ──────────────────────────────────────────────────

#[tauri::command]
async fn project_create(
    cwd: String,
    label: Option<String>,
    group_id: Option<String>,
    init_skills: Option<bool>,
) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    lazed::api_call(
        "project.create",
        json!({"cwd": cwd, "label": label, "group_id": group_id, "init_skills": init_skills}),
    )

    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn project_focus(project_id: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    lazed::api_call("project.focus", json!({"project_id": project_id}))

    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn project_close(project_id: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    lazed::api_call("project.close", json!({"project_id": project_id}))

    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn project_rename(project_id: String, label: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    lazed::api_call(
        "project.rename",
        json!({"project_id": project_id, "label": label}),
    )

    }).await.map_err(|e| e.to_string())?
}

// ── groups (named project collections, Orca-style sidebar sections) ──

#[tauri::command]
async fn group_create(label: Option<String>) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    lazed::api_call("group.create", json!({"label": label}))

    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn group_rename(group_id: String, label: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    lazed::api_call(
        "group.rename",
        json!({"group_id": group_id, "label": label}),
    )

    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn group_remove(group_id: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    lazed::api_call("group.remove", json!({"group_id": group_id}))

    }).await.map_err(|e| e.to_string())?
}

/// Move a project into a group — `group_id: null` ungroups it.
#[tauri::command]
async fn group_assign(project_id: String, group_id: Option<String>) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    lazed::api_call(
        "group.assign",
        json!({"project_id": project_id, "group_id": group_id}),
    )

    }).await.map_err(|e| e.to_string())?
}

/// New pane — `target_pane_id` splits next to an existing pane (herdr
/// `pane.split`), otherwise `workspace_id` opens a fresh tab with its root
/// pane. Both return the new herdr pane under `pane`.
#[tauri::command]
async fn pane_create(
    target_pane_id: Option<String>,
    workspace_id: Option<String>,
    cwd: Option<String>,
) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    if let Some(target) = target_pane_id {
        let r = lazed::herdr_call(
            "pane.split",
            json!({"target_pane_id": target, "direction": "right", "cwd": cwd}),
        )?;
        return Ok(json!({"pane": r.get("pane").cloned().unwrap_or(r)}));
    }
    let ws = workspace_id.ok_or("pane_create needs target_pane_id or workspace_id")?;
    let r = lazed::herdr_call("tab.create", json!({"workspace_id": ws, "cwd": cwd}))?;
    Ok(json!({"pane": r.get("root_pane").cloned().unwrap_or(Value::Null), "tab": r.get("tab").cloned().unwrap_or(Value::Null)}))

    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn pane_close(pane_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let control = state.control.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let handle = control.lock().map_err(|e| e.to_string())?.remove(&pane_id);
        if let Some(handle) = handle { handle.close(); }
        lazed::herdr_call("pane.close", json!({"pane_id": pane_id})).map(|_| ())
    }).await.map_err(|e| e.to_string())?
}

/// Generic herdr API passthrough — worktree create/remove and any other
/// herdr method reach herdr's socket from the UI as `{method, params}`;
/// no lazed-side reimplementation of herdr features.
#[tauri::command]
async fn herdr_call(method: String, params: Value) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    lazed::herdr_call(&method, params)

    }).await.map_err(|e| e.to_string())?
}

// ── workspaces (one checkout each) & tabs (pane rows inside them) ──

#[tauri::command]
async fn workspace_rename(workspace_id: String, label: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    lazed::api_call(
        "workspace.rename",
        json!({"workspace_id": workspace_id, "label": label}),
    )

    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn tab_create(workspace_id: String, label: Option<String>) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    lazed::herdr_call("tab.create", json!({"workspace_id": workspace_id, "label": label}))

    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn tab_close(tab_id: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    lazed::herdr_call("tab.close", json!({"tab_id": tab_id}))

    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn worktree_diff(checkout: String, base: Option<String>) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    git::worktree_diff(&checkout, base.as_deref())

    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn worktree_merge(repo: String, branch: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    git::worktree_merge(&repo, &branch)

    }).await.map_err(|e| e.to_string())?
}

/// Local branches of a repo — the new-worktree sheet's branch picker.
#[tauri::command]
async fn repo_branches(repo: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || git::repo_branches(&repo))
        .await
        .map_err(|e| e.to_string())?
}

/// Delete a branch in `repo` — the second half of worktree removal, which
/// herdr's `worktree.remove` deliberately leaves behind.
#[tauri::command]
async fn branch_delete(repo: String, branch: String, force: bool) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    git::branch_delete(&repo, &branch, force)

    }).await.map_err(|e| e.to_string())?
}

// ── files panel (right sidebar — checkout tree, badges, search) ────

/// Checkout file tree for the files panel: tracked + untracked with git
/// badges; ignored dirs appear collapsed and flagged.
#[tauri::command]
async fn fs_tree(root: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || fs::tree(&root))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn fs_watch(root: String, app: tauri::AppHandle, state: State<'_, AppState>) -> Result<Value, String> {
    let watches = state.watches.clone();
    tauri::async_runtime::spawn_blocking(move || watches.add(&root, move |event| { let _ = app.emit("fs.changed", event); }))
        .await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn fs_unwatch(watch_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let watches = state.watches.clone();
    tauri::async_runtime::spawn_blocking(move || watches.remove(&watch_id)).await.map_err(|e| e.to_string())?
}

/// Text content for the peek overlay (capped; binary flagged, not sent).
#[tauri::command]
async fn fs_read(path: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || fs::read_file(&path))
        .await
        .map_err(|e| e.to_string())?
}

/// Content search across the checkout — rg when installed, else git grep.
#[tauri::command]
async fn fs_search(root: String, query: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || fs::search(&root, &query))
        .await
        .map_err(|e| e.to_string())?
}

/// One file's diff vs HEAD — staged + unstaged; empty for clean files.
#[tauri::command]
async fn fs_diff(root: String, path: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || fs::file_diff(&root, &path))
        .await
        .map_err(|e| e.to_string())?
}

/// Open a file in the desktop's default app.
#[tauri::command]
fn open_path(path: String) -> Result<(), String> {
    fs::open_path(&path)
}

/// Repo identity for a path ({repo_key, repo_root, name} or null) — powers
/// project dedupe in the import flow.
#[tauri::command]
async fn resolve_repo(cwd: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    Ok(git::resolve_repo(&cwd)?.unwrap_or(Value::Null))

    }).await.map_err(|e| e.to_string())?
}

// ── agents (herdr's — names, readiness, lifecycle) ─────────────────

/// herdr needs a unique live name per agent; GUI launches derive one
/// from the kind and the pane (`claude-w1-p3`).
pub(crate) fn gui_agent_name(kind: &str, pane_id: &str) -> String {
    let mut name: String = format!("{kind}-{pane_id}")
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' { c } else { '-' })
        .collect();
    if !name.starts_with(|c: char| c.is_ascii_lowercase()) {
        name.insert(0, 'a');
    }
    name.truncate(32);
    name
}

#[tauri::command]
async fn agent_start(pane_id: String, kind: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || lazed::herdr_call(
        "agent.start",
        json!({"name": gui_agent_name(&kind, &pane_id), "kind": kind, "pane_id": pane_id}),
    )).await.map_err(|e| e.to_string())?
}

/// Submit a prompt; herdr refuses (`agent_blocked`/`agent_not_ready`)
/// before writing anything when the agent can't take input, so Ok means
/// the text + Enter were delivered.
#[tauri::command]
async fn agent_prompt(pane_id: String, text: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        lazed::herdr_call("agent.prompt", json!({"target": pane_id, "text": text}))
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn task_start(params: Value) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || lazed::api_call("task.start", params))
        .await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn agent_get(pane_id: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    lazed::herdr_call("agent.get", json!({"target": pane_id}))

    }).await.map_err(|e| e.to_string())?
}

/// Installed agent CLIs (Settings → Agents). Local `which` probe.
#[tauri::command]
async fn agent_detect() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(lazed::agent_detect)
        .await
        .map_err(|e| e.to_string())
}

/// CLI link state — the onboarding banner polls this on boot.
#[tauri::command]
async fn install_status() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(lazed::doctor_report)
        .await
        .map_err(|e| e.to_string())?
}

/// Link `~/.local/bin/lazed`. Runs the bundled (or
/// installed) binary; user consent is the banner button itself.
#[tauri::command]
async fn install_cli() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(lazed::cli_install)
        .await
        .map_err(|e| e.to_string())?
}

/// Type a line into a shell pane without a stream (prompt fan-out to
/// panes that aren't attached). Text + Enter as one ordered submission.
#[tauri::command]
async fn pane_send(pane_id: String, text: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    lazed::herdr_call(
        "pane.send_input",
        json!({"pane_id": pane_id, "text": text, "keys": ["enter"]}),
    )

    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn pane_read(pane_id: String, lines: Option<u32>) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    lazed::herdr_call(
        "pane.read",
        json!({"pane_id": pane_id, "source": "visible", "lines": lines.unwrap_or(50)}),
    )

    }).await.map_err(|e| e.to_string())?
}

/// Agent status notification with click-to-jump. The notification plugin's
/// `onAction` is mobile-only — on desktop it goes through notify-rust, so we
/// hold the handle on a thread and emit `notification.jump` on a body click.
#[tauri::command]
fn notify_agent(
    app: tauri::AppHandle,
    pane_id: String,
    project_id: Option<String>,
    title: String,
    body: String,
    sound: Option<String>,
) -> Result<(), String> {
    let mut n = notify_rust::Notification::new();
    n.appname("lazed").summary(&title).body(&body);
    if let Some(s) = sound.as_deref() {
        n.sound_name(s);
    }
    let handle = n.show().map_err(|e| e.to_string())?;
    std::thread::spawn(move || {
        handle.wait_for_action(move |action| {
            if action == "default" {
                let _ = app.emit(
                    "notification.jump",
                    json!({"pane_id": pane_id, "project_id": project_id}),
                );
            }
        });
    });
    Ok(())
}

// ── inbox (daemon-owned GTD capture queue) ─────────────────────────

#[tauri::command]
async fn inbox_list(all: Option<bool>) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    lazed::api_call("inbox.list", json!({"all": all.unwrap_or(false)}))

    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn inbox_add(params: Value) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    lazed::api_call("inbox.add", params)

    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn inbox_update(params: Value) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    lazed::api_call("inbox.update", params)

    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn inbox_remove(id: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    lazed::api_call("inbox.remove", json!({"id": id}))

    }).await.map_err(|e| e.to_string())?
}

// ── todo (daemon-owned personal checklist) ─────────────────────────

#[tauri::command]
async fn todo_list() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    lazed::api_call("todo.list", json!({}))

    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn todo_add(params: Value) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    lazed::api_call("todo.add", params)

    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn todo_update(params: Value) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    lazed::api_call("todo.update", params)

    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn todo_remove(id: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    lazed::api_call("todo.remove", json!({"id": id}))

    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn todo_clear() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    lazed::api_call("todo.clear", json!({}))

    }).await.map_err(|e| e.to_string())?
}

/// Open an item's source link in the desktop browser — http(s) only.
#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("refusing non-http(s) url".into());
    }
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(not(target_os = "macos"))]
    let opener = "xdg-open";
    std::process::Command::new(opener)
        .arg(&url)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

// ── automations ────────────────────────────────────────────────────

#[tauri::command]
async fn automation_list() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    automation::list()

    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn automation_save(input: Value) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    automation::save(input)

    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn automation_delete(id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
    automation::delete(&id)

    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn automation_set_enabled(id: String, enabled: bool) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
    automation::set_enabled(&id, enabled)

    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
fn automation_run_now(id: String) -> Result<(), String> {
    automation::run_now(id)
}

#[tauri::command]
async fn automation_reset_seen(id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
    automation::reset_seen(&id)

    }).await.map_err(|e| e.to_string())?
}

/// Fire one item's action — the agent path blocks for seconds, so it
/// runs off the IPC thread.
#[tauri::command]
async fn automation_fire(id: String, item_id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || automation::fire(&id, &item_id))
        .await
        .map_err(|e| e.to_string())?
}

/// Editor's dry-run: runs the poller once (up to its timeout).
#[tauri::command]
async fn automation_test(command: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || automation::test(&command))
        .await
        .map_err(|e| e.to_string())?
}

/// Editor's "requires" probe — bins resolved on the spawn PATH, env keys
/// present in the merged spawn env (user env or a stored integration).
#[tauri::command]
async fn deps_check(input: Value) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || Ok(env::deps_check(&input)))
        .await
        .map_err(|e| e.to_string())?
}

// ── integrations (connected accounts → env vars for spawned shells) ──

#[tauri::command]
async fn integration_list() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    integrations::list()

    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn integration_save(input: Value) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
    integrations::save(&input)

    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn integration_delete(provider: String, site: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
    integrations::delete(&provider, &site)

    }).await.map_err(|e| e.to_string())?
}

/// Probe a stored site's credentials — network call, off the IPC thread.
#[tauri::command]
async fn integration_test(provider: String, site: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || integrations::test(&provider, &site))
        .await
        .map_err(|e| e.to_string())?
}

/// Validate unsaved credentials for the connect modal — network call,
/// off the IPC thread.
#[tauri::command]
async fn integration_probe(input: Value) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || integrations::probe(&input))
        .await
        .map_err(|e| e.to_string())?
}

// ── events ─────────────────────────────────────────────────────────

#[tauri::command]
fn subscribe_events(on_event: Channel<Box<RawValue>>, state: State<AppState>) -> Result<(), String> {
    // always swap in the caller's channel — after a webview reload the old
    // channel is dead, and without this the stream keeps sending into it
    let chan_slot = state.events_channel.clone();
    *chan_slot.lock().map_err(|e| e.to_string())? = Some(on_event);
    let send = move |v: Box<RawValue>| {
        if let Ok(g) = chan_slot.lock() {
            if let Some(c) = g.as_ref() {
                let _ = c.send(v);
            }
        }
    };
    // only one subscription loop at a time
    {
        let mut flag = state.events_running.lock().map_err(|e| e.to_string())?;
        if *flag {
            return Ok(());
        }
        *flag = true;
    }
    std::thread::spawn(move || loop {
        let err = lazed::run_event_stream(|ev| {
            send(ev);
            true
        });
        eprintln!("[lazed] event stream ended: {err}; reconnecting");
        send(raw_json(&json!({"event": "events.reconnect", "type": "events.reconnect"})));
        std::thread::sleep(std::time::Duration::from_millis(800));
        let _ = lazed::ensure_server();
    });
    Ok(())
}

pub fn run() {
    // One channel slot shared by the daemon event stream and the automation
    // engine — subscribe_events writes the live Channel into it.
    let events_slot: Arc<Mutex<Option<Channel<Box<RawValue>>>>> = Arc::new(Mutex::new(None));
    let events_for_setup = events_slot.clone();
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            // Initialize before any notification can take the library's
            // fallback path, which asks AppleScript to find "use_default".
            #[cfg(target_os = "macos")]
            {
                let identifier = if tauri::is_dev() {
                    "com.apple.Terminal"
                } else {
                    &app.config().identifier
                };
                if let Err(err) = notify_rust::set_application(identifier) {
                    eprintln!("[lazed] notification sender initialization failed: {err}");
                }
            }
            // Prefer a bundled lazed binary when the package ships one
            // (bundle.resources → src-tauri/bin/lazed). Dev runs register
            // nothing → PATH lookup finds ~/.local/bin/lazed.
            let own_exe = std::env::current_exe().ok();
            let bundled = app.path().resource_dir().ok().and_then(|dir| {
                [dir.join("lazed"), dir.join("bin").join("lazed")]
                    .into_iter()
                    .find(|p| p.is_file() && Some(p) != own_exe.as_ref())
            });
            if let Some(p) = &bundled {
                eprintln!("[lazed] using bundled lazed: {}", p.display());
            }
            lazed::register_bundled(bundled);
            // bundled herdr (bundle.resources → bin/herdr) — the daemon gets
            // the same path via HERDR_BIN when the app spawns it
            let bundled_herdr = app
                .path()
                .resource_dir()
                .ok()
                .map(|dir| dir.join("bin").join("herdr"))
                .filter(|p| p.is_file());
            if let Some(p) = &bundled_herdr {
                eprintln!("[lazed] using bundled herdr: {}", p.display());
            }
            herdr::register_bundled(bundled_herdr);
            env::warmup();
            let config_dir = app
                .path()
                .app_config_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            integrations::init(config_dir);
            automation::init(app.handle().clone(), events_for_setup);
            Ok(())
        })
        .manage(AppState {
            watches: Arc::new(fs_watch::Watches::default()),
            control: Arc::new(Mutex::new(HashMap::new())),
            events_running: Mutex::new(false),
            events_channel: events_slot,
        })
        .invoke_handler(tauri::generate_handler![
            bootstrap,
            herdr_status,
            session_snapshot,
            session_status,
            server_restart,
            pane_attach,
            pane_input,
            pane_resize,
            pane_scroll,
            pane_detach,
            pane_close,
            pane_create,
            pane_send,
            pane_read,
            notify_agent,
            project_create,
            project_focus,
            project_close,
            project_rename,
            group_create,
            group_rename,
            group_remove,
            group_assign,
            herdr_call,
            workspace_rename,
            tab_create,
            tab_close,
            worktree_diff,
            worktree_merge,
            repo_branches,
            branch_delete,
            fs_tree,
            fs_watch,
            fs_unwatch,
            fs_read,
            fs_search,
            fs_diff,
            open_path,
            resolve_repo,
            agent_start,
            task_start,
            agent_prompt,
            agent_get,
            agent_detect,
            install_status,
            install_cli,
            subscribe_events,
            automation_list,
            automation_save,
            automation_delete,
            automation_set_enabled,
            automation_run_now,
            automation_reset_seen,
            automation_fire,
            automation_test,
            deps_check,
            integration_list,
            integration_save,
            integration_delete,
            integration_test,
            integration_probe,
            inbox_list,
            inbox_add,
            inbox_update,
            inbox_remove,
            todo_list,
            todo_add,
            todo_update,
            todo_remove,
            todo_clear,
            open_url,
        ])
        .build(tauri::generate_context!())
        .unwrap_or_else(|err| {
            eprintln!("error while building lazed: {err}");
            std::process::exit(1);
        });
    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            // stop the herdr stream children — panes live on in herdr
            if let Some(state) = handle.try_state::<AppState>() {
                state.watches.clear();
                if let Ok(mut map) = state.control.lock() {
                    for (_, h) in map.drain() {
                        h.close();
                    }
                }
            }
        }
    });
}
