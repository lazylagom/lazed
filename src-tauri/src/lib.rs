mod git;
mod herdr;

use std::collections::HashMap;
use std::io::BufRead;
use std::sync::Mutex;

use serde_json::Value;
use tauri::ipc::Channel;
use tauri::State;

struct AppState {
    control: Mutex<HashMap<String, herdr::ControlHandle>>,
    events_running: Mutex<bool>,
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
    // drop stale control sessions whose pane disappeared
    let live: Vec<String> = panes
        .iter()
        .filter_map(|p| p.get("pane_id").and_then(Value::as_str).map(str::to_string))
        .collect();
    state
        .control
        .lock()
        .map_err(|e| e.to_string())?
        .retain(|id, _| live.contains(id));
    Ok(serde_json::json!({"workspace": workspace, "panes": panes}))
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
fn agent_start(pane_id: String, kind: String, name: Option<String>) -> Result<Value, String> {
    herdr::agent_start(&pane_id, &kind, name.as_deref())
}

#[tauri::command]
fn agent_prompt(pane_id: String, text: String) -> Result<Value, String> {
    herdr::agent_prompt(&pane_id, &text)
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

#[tauri::command]
async fn remote_connect(target: String, state: State<'_, AppState>) -> Result<(), String> {
    // drop all local control streams before switching context
    if let Ok(mut map) = state.control.lock() {
        for (_, mut h) in map.drain() {
            let _ = h.child.kill();
        }
    }
    // ssh probing + forward setup blocks for seconds — keep it off the
    // webview's command path
    tauri::async_runtime::spawn_blocking(move || herdr::remote_connect(&target))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
fn remote_disconnect(state: State<AppState>) -> Result<(), String> {
    if let Ok(mut map) = state.control.lock() {
        for (_, mut h) in map.drain() {
            let _ = h.child.kill();
        }
    }
    herdr::remote_disconnect()
}

#[tauri::command]
fn remote_status() -> Result<Value, String> {
    Ok(serde_json::json!({"target": herdr::remote_target()}))
}

#[tauri::command]
fn subscribe_events(
    on_event: Channel<Value>,
    state: State<AppState>,
) -> Result<(), String> {
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
                let _ = on_event.send(ev.clone());
            });
            eprintln!("[staylazy] event stream ended: {err}; reconnecting");
            let _ = on_event.send(serde_json::json!({"type": "events.reconnect"}));
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
        }
    }
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .manage(AppState {
            control: Mutex::new(HashMap::new()),
            events_running: Mutex::new(false),
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
            tab_create,
            tab_focus,
            tab_close,
            agent_start,
            agent_prompt,
            pane_send_text,
            worktree_create,
            worktree_list,
            worktree_remove,
            worktree_diff,
            worktree_merge,
            remote_connect,
            remote_disconnect,
            remote_status,
            subscribe_events,
        ])
        .run(tauri::generate_context!())
        .unwrap_or_else(|err| eprintln!("error while running staylazy: {err}"));
}
