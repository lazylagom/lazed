mod git;
mod herdr;

use std::collections::HashMap;
use std::io::BufRead;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tauri::ipc::Channel;
use tauri::{Manager, State};

struct AppState {
    control: Mutex<HashMap<String, herdr::ControlHandle>>,
    events_running: Mutex<bool>,
    /// The live webview's event channel. A reload re-invokes subscribe_events
    /// with a fresh Channel — the stream thread must send to the newest one.
    events_channel: Arc<Mutex<Option<Channel<Value>>>>,
}

#[tauri::command]
fn bootstrap(state: State<AppState>) -> Result<Value, String> {
    herdr::ensure_server()?;
    // remote: let the remote server pick the cwd (local $HOME may not exist)
    let cwd = if herdr::remote_target().is_some() {
        None
    } else {
        Some(std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string()))
    };
    let workspace = herdr::ensure_workspace(cwd.as_deref())?;
    let panes = herdr::list_panes()?;
    // drop stale control sessions whose pane disappeared — kill + reap so
    // the child (or its ssh pipe) doesn't leak as a zombie/orphan
    let live: Vec<String> = panes
        .iter()
        .filter_map(|p| p.get("pane_id").and_then(Value::as_str).map(str::to_string))
        .collect();
    if let Ok(mut map) = state.control.lock() {
        let stale: Vec<String> = map
            .keys()
            .filter(|id| !live.contains(id))
            .cloned()
            .collect();
        for id in stale {
            if let Some(mut h) = map.remove(&id) {
                let _ = h.child.kill();
                let _ = h.child.wait();
            }
        }
    }
    Ok(serde_json::json!({
        "workspace": workspace,
        "panes": panes,
        "herdr_warning": herdr::compat_warning(),
    }))
}

#[tauri::command]
fn list_panes() -> Result<Value, String> {
    Ok(serde_json::json!({"panes": herdr::list_panes()?}))
}

#[tauri::command]
fn attach_pane(
    pane_id: String,
    cols: u32,
    rows: u32,
    on_frame: Channel<Value>,
    state: State<AppState>,
) -> Result<(), String> {
    // detach existing control session for this pane
    detach_pane_internal(&state, &pane_id);

    let (mut child, mut reader) = herdr::open_control_stream(&pane_id, cols, rows)?;
    let stdin = herdr::take_stdin(&mut child)?;
    state
        .control
        .lock()
        .map_err(|e| e.to_string())?
        .insert(pane_id.clone(), herdr::ControlHandle { child, stdin });

    let pid = pane_id.clone();
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
                    match serde_json::from_str::<Value>(trimmed) {
                        Ok(v) => {
                            if on_frame.send(v).is_err() {
                                break;
                            }
                        }
                        Err(_) => {
                            let _ = on_frame.send(serde_json::json!({
                                "type": "log", "text": trimmed
                            }));
                        }
                    }
                }
            }
        }
        let _ = on_frame.send(serde_json::json!({"type": "terminal.closed"}));
        eprintln!("[staylazy] control stream ended for {pid}");
    });
    Ok(())
}

#[tauri::command]
fn pane_input(pane_id: String, text: String, state: State<AppState>) -> Result<(), String> {
    let mut map = state.control.lock().map_err(|e| e.to_string())?;
    let handle = map
        .get_mut(&pane_id)
        .ok_or_else(|| format!("no control stream for {pane_id}"))?;
    handle.send(&herdr::cmd_input_text(&text))
}

#[tauri::command]
fn pane_resize(
    pane_id: String,
    cols: u32,
    rows: u32,
    state: State<AppState>,
) -> Result<(), String> {
    let mut map = state.control.lock().map_err(|e| e.to_string())?;
    let handle = map
        .get_mut(&pane_id)
        .ok_or_else(|| format!("no control stream for {pane_id}"))?;
    handle.send(&herdr::cmd_resize(cols, rows))
}

#[tauri::command]
fn split_pane(pane_id: String, direction: String) -> Result<Value, String> {
    herdr::split_pane(&pane_id, &direction)
}

#[tauri::command]
fn session_snapshot() -> Result<Value, String> {
    herdr::snapshot()
}

#[tauri::command]
fn resize_pane(pane_id: String, direction: String, amount: f64) -> Result<Value, String> {
    herdr::resize_pane(&pane_id, &direction, amount)
}

#[tauri::command]
fn workspace_create(cwd: Option<String>, label: Option<String>) -> Result<Value, String> {
    herdr::workspace_create(cwd.as_deref(), label.as_deref())
}

#[tauri::command]
fn workspace_focus(workspace_id: String) -> Result<Value, String> {
    herdr::workspace_focus(&workspace_id)
}

#[tauri::command]
fn workspace_close(workspace_id: String) -> Result<Value, String> {
    herdr::workspace_close(&workspace_id)
}

#[tauri::command]
fn workspace_rename(workspace_id: String, label: String) -> Result<Value, String> {
    herdr::workspace_rename(&workspace_id, &label)
}

#[tauri::command]
fn tab_create(workspace_id: String, cwd: Option<String>) -> Result<Value, String> {
    herdr::tab_create(&workspace_id, cwd.as_deref())
}

#[tauri::command]
fn tab_focus(tab_id: String) -> Result<Value, String> {
    herdr::tab_focus(&tab_id)
}

#[tauri::command]
fn tab_close(tab_id: String) -> Result<Value, String> {
    herdr::tab_close(&tab_id)
}

#[tauri::command]
fn tab_rename(tab_id: String, label: String) -> Result<Value, String> {
    herdr::tab_rename(&tab_id, &label)
}

#[tauri::command]
fn agent_start(pane_id: String, kind: String, name: Option<String>) -> Result<Value, String> {
    herdr::agent_start(&pane_id, &kind, name.as_deref())
}

#[tauri::command]
fn agent_prompt(pane_id: String, text: String) -> Result<Value, String> {
    herdr::agent_prompt(&pane_id, &text)
}

#[tauri::command]
fn agent_get(pane_id: String) -> Result<Value, String> {
    herdr::agent_get(&pane_id)
}

#[tauri::command]
fn pane_send_text(pane_id: String, text: String) -> Result<Value, String> {
    herdr::pane_send_text(&pane_id, &text)
}

#[tauri::command]
fn worktree_create(
    cwd: String,
    branch: Option<String>,
    base: Option<String>,
    label: Option<String>,
    workspace: Option<String>,
) -> Result<Value, String> {
    herdr::worktree_create(
        &cwd,
        branch.as_deref(),
        base.as_deref(),
        label.as_deref(),
        workspace.as_deref(),
    )
}

#[tauri::command]
fn worktree_list(cwd: Option<String>) -> Result<Value, String> {
    herdr::worktree_list(cwd.as_deref())
}

#[tauri::command]
fn worktree_remove(workspace_id: String, force: bool) -> Result<Value, String> {
    herdr::worktree_remove(&workspace_id, force)
}

#[tauri::command]
fn worktree_diff(checkout: String, base: Option<String>) -> Result<Value, String> {
    git::worktree_diff(&checkout, base.as_deref())
}

#[tauri::command]
fn worktree_merge(repo: String, branch: String) -> Result<Value, String> {
    git::worktree_merge(&repo, &branch)
}

/// Kill every pane control stream. Streams are bound to whatever herdr
/// context was active when they attached, so they must not outlive a context
/// switch — PaneView's attach retry re-binds them to the current target.
fn drain_control_streams(state: &State<AppState>) {
    if let Ok(mut map) = state.control.lock() {
        for (_, mut h) in map.drain() {
            let _ = h.child.kill();
            let _ = h.child.wait();
        }
    }
}

#[tauri::command]
async fn remote_connect(
    target: String,
    session: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    // ssh probing + forward setup blocks for seconds — keep it off the
    // webview's command path
    tauri::async_runtime::spawn_blocking(move || {
        herdr::remote_connect(&target, session.as_deref())
    })
    .await
    .map_err(|e| e.to_string())??;
    // Drain only after a successful attach: killing streams earlier would let
    // pane attach-retries rebind streams of the *old* context mid-probe, which
    // would then survive the switch (colliding pane ids) and keep serving the
    // old machine under the new context. On failure nothing is touched.
    drain_control_streams(&state);
    Ok(())
}

#[tauri::command]
fn remote_disconnect(state: State<AppState>) -> Result<(), String> {
    // clear REMOTE first so post-kill attach retries bind locally
    herdr::remote_disconnect()?;
    drain_control_streams(&state);
    Ok(())
}

#[tauri::command]
fn remote_status() -> Result<Value, String> {
    let (target, session) = herdr::remote_ctx().unwrap_or((String::new(), None));
    Ok(serde_json::json!({
        "target": if target.is_empty() { None } else { Some(target) },
        "session": session,
    }))
}

/// Re-check the ACTIVE server's compat status (local, or the attached
/// remote) — bootstrap alone can't see a context that attached later.
#[tauri::command]
fn compat_warning() -> Option<String> {
    herdr::compat_warning()
}

#[tauri::command]
fn machine_list() -> Result<Value, String> {
    herdr::machine_list()
}

#[tauri::command]
fn machine_remove(id: String) -> Result<(), String> {
    herdr::machine_remove(&id)
}

#[tauri::command]
fn machine_rename(id: String, label: String) -> Result<(), String> {
    herdr::machine_rename(&id, &label)
}

#[tauri::command]
fn subscribe_events(
    on_event: Channel<Value>,
    state: State<AppState>,
) -> Result<(), String> {
    // always swap in the caller's channel — after a webview reload the old
    // channel is dead, and without this the stream keeps sending into it
    let chan_slot = state.events_channel.clone();
    *chan_slot.lock().map_err(|e| e.to_string())? = Some(on_event);
    let send = move |v: &Value| {
        if let Ok(g) = chan_slot.lock() {
            if let Some(c) = g.as_ref() {
                let _ = c.send(v.clone());
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
    std::thread::spawn(move || {
        loop {
            let pane_ids: Vec<String> = herdr::snapshot()
                .ok()
                .and_then(|s| {
                    s.pointer("/panes").and_then(Value::as_array).map(|p| {
                        p.iter()
                            .filter_map(|x| x.get("pane_id").and_then(Value::as_str).map(str::to_string))
                            .collect()
                    })
                })
                .unwrap_or_default();

            let mut subscribed: std::collections::HashSet<String> =
                pane_ids.iter().cloned().collect();
            let err = herdr::run_event_stream(&pane_ids, |ev, add_sub| {
                // dynamically subscribe to status changes for newly created panes
                // (event names arrive as "pane_created" or "pane.created")
                let name = ev
                    .pointer("/event")
                    .or_else(|| ev.pointer("/data/type"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .replace('.', "_");
                if name == "pane_created" {
                    if let Some(id) = ev
                        .pointer("/data/pane/pane_id")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                    {
                        if subscribed.insert(id.clone()) {
                            add_sub(&herdr::status_sub_for_pane(&id));
                        }
                    }
                }
                send(ev);
            });
            eprintln!("[staylazy] event stream ended: {err}; reconnecting");
            // frontend reads the name from `event` (or data.type) — send both
            send(&serde_json::json!({
                "event": "events.reconnect", "type": "events.reconnect"
            }));
            std::thread::sleep(std::time::Duration::from_millis(800));
            let _ = herdr::ensure_server();
        }
    });
    Ok(())
}

#[tauri::command]
fn run_in_pane(pane_id: String, command: String) -> Result<Value, String> {
    herdr::run_in_pane(&pane_id, &command)
}

#[tauri::command]
fn close_pane(pane_id: String, state: State<AppState>) -> Result<(), String> {
    detach_pane_internal(&state, &pane_id);
    herdr::close_pane(&pane_id).map(|_| ())
}

#[tauri::command]
fn detach_pane(pane_id: String, state: State<AppState>) -> Result<(), String> {
    detach_pane_internal(&state, &pane_id);
    Ok(())
}

fn detach_pane_internal(state: &State<AppState>, pane_id: &str) {
    if let Ok(mut map) = state.control.lock() {
        if let Some(mut handle) = map.remove(pane_id) {
            let _ = handle.send(&serde_json::json!({"type": "terminal.release"}));
            let _ = handle.child.kill();
            let _ = handle.child.wait();
        }
    }
}

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // Prefer the bundled herdr binary when the package ships one
            // (bundle.resources → src-tauri/bin/herdr, staged by
            // scripts/fetch-herdr). Dev runs register nothing → PATH lookup.
            let bundled = app.path().resource_dir().ok().and_then(|dir| {
                [dir.join("herdr"), dir.join("bin").join("herdr")]
                    .into_iter()
                    .find(|p| p.is_file())
            });
            if let Some(p) = &bundled {
                eprintln!("[staylazy] using bundled herdr: {}", p.display());
            }
            herdr::register_bundled(bundled);
            Ok(())
        })
        .manage(AppState {
            control: Mutex::new(HashMap::new()),
            events_running: Mutex::new(false),
            events_channel: Arc::new(Mutex::new(None)),
        })
        .invoke_handler(tauri::generate_handler![
            bootstrap,
            list_panes,
            attach_pane,
            pane_input,
            pane_resize,
            split_pane,
            run_in_pane,
            close_pane,
            detach_pane,
            session_snapshot,
            resize_pane,
            workspace_create,
            workspace_focus,
            workspace_close,
            workspace_rename,
            tab_create,
            tab_focus,
            tab_close,
            tab_rename,
            agent_start,
            agent_prompt,
            agent_get,
            pane_send_text,
            worktree_create,
            worktree_list,
            worktree_remove,
            worktree_diff,
            worktree_merge,
            remote_connect,
            remote_disconnect,
            remote_status,
            machine_list,
            machine_remove,
            machine_rename,
            compat_warning,
            subscribe_events,
        ])
        .build(tauri::generate_context!())
        .unwrap_or_else(|err| {
            eprintln!("error while building staylazy: {err}");
            std::process::exit(1);
        });
    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            // kill the ssh forward + control children — dropping a Child does
            // not terminate the process, so they would outlive the app
            if let Some(state) = handle.try_state::<AppState>() {
                if let Ok(mut map) = state.control.lock() {
                    for (_, mut h) in map.drain() {
                        let _ = h.child.kill();
                        let _ = h.child.wait();
                    }
                }
            }
            let _ = herdr::remote_disconnect();
        }
    });
}
