mod herdr;

use std::collections::HashMap;
use std::io::BufRead;
use std::sync::Mutex;

use serde_json::Value;
use tauri::ipc::Channel;
use tauri::State;

struct AppState {
    control: Mutex<HashMap<String, herdr::ControlHandle>>,
}

#[tauri::command]
fn bootstrap(state: State<AppState>) -> Result<Value, String> {
    herdr::ensure_server()?;
    let cwd = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let workspace = herdr::ensure_workspace(&cwd)?;
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
        .manage(AppState {
            control: Mutex::new(HashMap::new()),
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
        ])
        .run(tauri::generate_context!())
        .unwrap_or_else(|err| eprintln!("error while running staylazy: {err}"));
}
