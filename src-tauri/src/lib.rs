mod automation;
mod env;
mod git;
mod integrations;
mod lazed;

use std::collections::HashMap;
use std::io::BufRead;
use std::sync::{Arc, Mutex};

use serde_json::value::RawValue;
use serde_json::{json, Value};
use tauri::ipc::Channel;
use tauri::{Emitter, Manager, State};

struct AppState {
    control: Mutex<HashMap<String, lazed::ControlHandle>>,
    events_running: Mutex<bool>,
    /// The live webview's event channel. A reload re-invokes
    /// subscribe_events with a fresh Channel — the stream thread must
    /// send to the newest one.
    events_channel: Arc<Mutex<Option<Channel<Value>>>>,
}

#[tauri::command]
fn bootstrap(state: State<AppState>) -> Result<Value, String> {
    lazed::ensure_server()?;
    let snap = lazed::api_call("session.snapshot", json!({}))?;
    // drop stale control streams whose terminal disappeared — the daemon
    // unsubscribes on connection EOF
    let live: Vec<String> = snap
        .pointer("/terminals")
        .and_then(Value::as_array)
        .map(|ts| {
            ts.iter()
                .filter_map(|t| t.get("term_id").and_then(Value::as_str).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if let Ok(mut map) = state.control.lock() {
        let stale: Vec<String> = map
            .keys()
            .filter(|id| !live.contains(id))
            .cloned()
            .collect();
        for id in stale {
            if let Some(h) = map.remove(&id) {
                let _ = h.stream.shutdown(std::net::Shutdown::Both);
            }
        }
    }
    Ok(json!({"snapshot": snap}))
}

#[tauri::command]
fn session_snapshot() -> Result<Value, String> {
    lazed::api_call("session.snapshot", json!({}))
}

#[tauri::command]
fn session_status() -> Result<Value, String> {
    lazed::api_call("session.status", json!({}))
}

#[tauri::command]
async fn server_restart() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(lazed::restart_server)
        .await
        .map_err(|e| e.to_string())?
}

// ── terminal control streams ───────────────────────────────────────

#[tauri::command]
fn term_attach(
    term_id: String,
    cols: u32,
    rows: u32,
    on_frame: Channel<Box<RawValue>>,
    state: State<AppState>,
) -> Result<(), String> {
    detach_term_internal(&state, &term_id);

    let (stream, mut reader) = lazed::open_attach_stream(&term_id, cols, rows)?;
    state
        .control
        .lock()
        .map_err(|e| e.to_string())?
        .insert(term_id.clone(), lazed::ControlHandle { stream });

    let tid = term_id.clone();
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
        let _ = on_frame.send(raw_json(&json!({"type": "term.closed"})));
        eprintln!("[lazed] attach stream ended for {tid}");
    });
    Ok(())
}

/// Serialize a Value into a channel payload — json! output is always
/// valid JSON so validation cannot fail.
fn raw_json(v: &Value) -> Box<RawValue> {
    RawValue::from_string(v.to_string()).expect("serialized json! is valid")
}

fn send_control(state: &State<AppState>, term_id: &str, msg: &Value) -> Result<(), String> {
    let mut map = state.control.lock().map_err(|e| e.to_string())?;
    let handle = map
        .get_mut(term_id)
        .ok_or_else(|| format!("no control stream for {term_id}"))?;
    handle.send(msg)
}

#[tauri::command]
fn term_input(term_id: String, text: String, state: State<AppState>) -> Result<(), String> {
    send_control(&state, &term_id, &lazed::cmd_input_text(&text))
}

#[tauri::command]
fn term_resize(term_id: String, cols: u32, rows: u32, state: State<AppState>) -> Result<(), String> {
    send_control(&state, &term_id, &lazed::cmd_resize(cols, rows))
}

#[tauri::command]
fn term_scroll(
    term_id: String,
    delta_px: Option<f64>,
    cell_px: Option<f64>,
    delta_lines: Option<f64>,
    offset_from_bottom: Option<u32>,
    column: u16,
    row: u16,
    modifiers: u8,
    state: State<AppState>,
) -> Result<(), String> {
    let msg = if let Some(off) = offset_from_bottom {
        lazed::cmd_scroll_to(off)
    } else if let (Some(px), Some(cell)) = (delta_px, cell_px) {
        lazed::cmd_scroll_px(px, cell, column, row, modifiers)
    } else {
        lazed::cmd_scroll_lines(delta_lines.unwrap_or(0.0), column, row, modifiers)
    };
    send_control(&state, &term_id, &msg)
}

#[tauri::command]
fn term_detach(term_id: String, state: State<AppState>) -> Result<(), String> {
    detach_term_internal(&state, &term_id);
    Ok(())
}

fn detach_term_internal(state: &State<AppState>, term_id: &str) {
    if let Ok(mut map) = state.control.lock() {
        if let Some(mut handle) = map.remove(term_id) {
            let _ = handle.send(&json!({"type": "detach"}));
            // EOF on the conn unsubscribes server-side too — shutdown
            // covers a daemon that doesn't act on the detach command.
            let _ = handle.stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

// ── model methods ──────────────────────────────────────────────────

#[tauri::command]
fn project_create(
    cwd: String,
    label: Option<String>,
    group_id: Option<String>,
) -> Result<Value, String> {
    lazed::api_call(
        "project.create",
        json!({"cwd": cwd, "label": label, "group_id": group_id}),
    )
}

#[tauri::command]
fn project_focus(project_id: String) -> Result<Value, String> {
    lazed::api_call("project.focus", json!({"project_id": project_id}))
}

#[tauri::command]
fn project_close(project_id: String) -> Result<Value, String> {
    lazed::api_call("project.close", json!({"project_id": project_id}))
}

#[tauri::command]
fn project_rename(project_id: String, label: String) -> Result<Value, String> {
    lazed::api_call(
        "project.rename",
        json!({"project_id": project_id, "label": label}),
    )
}

// ── groups (named project collections, Orca-style sidebar sections) ──

#[tauri::command]
fn group_create(label: Option<String>) -> Result<Value, String> {
    lazed::api_call("group.create", json!({"label": label}))
}

#[tauri::command]
fn group_rename(group_id: String, label: String) -> Result<Value, String> {
    lazed::api_call(
        "group.rename",
        json!({"group_id": group_id, "label": label}),
    )
}

#[tauri::command]
fn group_remove(group_id: String) -> Result<Value, String> {
    lazed::api_call("group.remove", json!({"group_id": group_id}))
}

/// Move a project into a group — `group_id: null` ungroups it.
#[tauri::command]
fn group_assign(project_id: String, group_id: Option<String>) -> Result<Value, String> {
    lazed::api_call(
        "group.assign",
        json!({"project_id": project_id, "group_id": group_id}),
    )
}

/// New pane — `tab_id` splits into an existing tab, `workspace_id` /
/// `project_id` open a fresh tab under them.
#[tauri::command]
fn term_create(
    tab_id: Option<String>,
    workspace_id: Option<String>,
    project_id: Option<String>,
    command: Option<String>,
    label: Option<String>,
) -> Result<Value, String> {
    lazed::api_call(
        "terminal.create",
        json!({
            "tab_id": tab_id,
            "workspace_id": workspace_id,
            "project_id": project_id,
            "command": command,
            "label": label,
        }),
    )
}

#[tauri::command]
fn term_close(term_id: String, state: State<AppState>) -> Result<(), String> {
    detach_term_internal(&state, &term_id);
    lazed::api_call("terminal.close", json!({"term_id": term_id})).map(|_| ())
}

// ── workspaces (one checkout each) & tabs (pane rows inside them) ──

#[tauri::command]
fn workspace_create(
    project_id: String,
    branch: String,
    base: Option<String>,
    label: Option<String>,
) -> Result<Value, String> {
    lazed::api_call(
        "workspace.create",
        json!({"project_id": project_id, "branch": branch, "base": base, "label": label}),
    )
}

#[tauri::command]
fn workspace_remove(workspace_id: String, force: bool) -> Result<Value, String> {
    lazed::api_call(
        "workspace.remove",
        json!({"workspace_id": workspace_id, "force": force}),
    )
}

#[tauri::command]
fn workspace_rename(workspace_id: String, label: String) -> Result<Value, String> {
    lazed::api_call(
        "workspace.rename",
        json!({"workspace_id": workspace_id, "label": label}),
    )
}

#[tauri::command]
fn tab_create(
    workspace_id: String,
    label: Option<String>,
    command: Option<String>,
) -> Result<Value, String> {
    lazed::api_call(
        "tab.create",
        json!({"workspace_id": workspace_id, "label": label, "command": command}),
    )
}

#[tauri::command]
fn tab_close(tab_id: String) -> Result<Value, String> {
    lazed::api_call("tab.close", json!({"tab_id": tab_id}))
}

#[tauri::command]
fn worktree_diff(checkout: String, base: Option<String>) -> Result<Value, String> {
    git::worktree_diff(&checkout, base.as_deref())
}

#[tauri::command]
fn worktree_merge(repo: String, branch: String) -> Result<Value, String> {
    git::worktree_merge(&repo, &branch)
}

/// Repo identity for a path ({repo_key, repo_root, name} or null) — powers
/// project dedupe in the import flow.
#[tauri::command]
fn resolve_repo(cwd: String) -> Result<Value, String> {
    Ok(git::resolve_repo(&cwd)?.unwrap_or(Value::Null))
}

// ── agents ─────────────────────────────────────────────────────────

#[tauri::command]
async fn agent_start(term_id: String, kind: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || lazed::api_call(
        "agent.start",
        json!({"term_id": term_id, "kind": kind}),
    )).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn agent_prompt(term_id: String, text: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let receipt = lazed::api_call(
        "agent.prompt",
        json!({"term_id": term_id, "text": text}),
        )?;
        if receipt["accepted"] != true { return Err(format!("{}", receipt["error"])); }
        Ok(receipt)
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn task_start(params: Value) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || lazed::api_call("task.start", params))
        .await.map_err(|e| e.to_string())?
}

#[tauri::command]
fn agent_get(term_id: String) -> Result<Value, String> {
    lazed::api_call("agent.get", json!({"term_id": term_id}))
}

/// Installed agent CLIs (Settings → Agents). Local `which` probe.
#[tauri::command]
async fn agent_detect() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(lazed::agent_detect)
        .await
        .map_err(|e| e.to_string())
}

/// CLI/skill link state — the onboarding banner polls this on boot.
#[tauri::command]
async fn install_status() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(lazed::doctor_report)
        .await
        .map_err(|e| e.to_string())?
}

/// Link `~/.local/bin/lazed` + the agent-skill dirs. Runs the bundled (or
/// installed) binary; user consent is the banner button itself.
#[tauri::command]
async fn install_cli() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(lazed::cli_install)
        .await
        .map_err(|e| e.to_string())?
}

/// Send raw text into a terminal without a control stream (one-shot —
/// used by prompt fan-out to terminals that aren't attached).
#[tauri::command]
fn term_send(term_id: String, text: String) -> Result<Value, String> {
    lazed::api_call(
        "terminal.input",
        json!({"term_id": term_id, "text": text}),
    )
}

#[tauri::command]
fn term_read(term_id: String, lines: Option<u32>) -> Result<Value, String> {
    lazed::api_call(
        "terminal.read",
        json!({"term_id": term_id, "lines": lines.unwrap_or(50)}),
    )
}

/// Agent status notification with click-to-jump. The notification plugin's
/// `onAction` is mobile-only — on desktop it goes through notify-rust, so we
/// hold the handle on a thread and emit `notification.jump` on a body click.
#[tauri::command]
fn notify_agent(
    app: tauri::AppHandle,
    term_id: String,
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
                    json!({"term_id": term_id, "project_id": project_id}),
                );
            }
        });
    });
    Ok(())
}

// ── inbox (daemon-owned GTD capture queue) ─────────────────────────

#[tauri::command]
fn inbox_list(all: Option<bool>) -> Result<Value, String> {
    lazed::api_call("inbox.list", json!({"all": all.unwrap_or(false)}))
}

#[tauri::command]
fn inbox_add(params: Value) -> Result<Value, String> {
    lazed::api_call("inbox.add", params)
}

#[tauri::command]
fn inbox_update(params: Value) -> Result<Value, String> {
    lazed::api_call("inbox.update", params)
}

#[tauri::command]
fn inbox_remove(id: String) -> Result<Value, String> {
    lazed::api_call("inbox.remove", json!({"id": id}))
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
fn automation_list() -> Result<Value, String> {
    automation::list()
}

#[tauri::command]
fn automation_save(input: Value) -> Result<Value, String> {
    automation::save(input)
}

#[tauri::command]
fn automation_delete(id: String) -> Result<(), String> {
    automation::delete(&id)
}

#[tauri::command]
fn automation_set_enabled(id: String, enabled: bool) -> Result<(), String> {
    automation::set_enabled(&id, enabled)
}

#[tauri::command]
fn automation_run_now(id: String) {
    automation::run_now(id)
}

#[tauri::command]
fn automation_reset_seen(id: String) -> Result<(), String> {
    automation::reset_seen(&id)
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
fn integration_list() -> Result<Value, String> {
    integrations::list()
}

#[tauri::command]
fn integration_save(input: Value) -> Result<Value, String> {
    integrations::save(&input)
}

#[tauri::command]
fn integration_delete(provider: String, site: String) -> Result<(), String> {
    integrations::delete(&provider, &site)
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
fn subscribe_events(on_event: Channel<Value>, state: State<AppState>) -> Result<(), String> {
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
    std::thread::spawn(move || loop {
        let err = lazed::run_event_stream(|ev| {
            send(ev);
            true
        });
        eprintln!("[lazed] event stream ended: {err}; reconnecting");
        send(&json!({"event": "events.reconnect", "type": "events.reconnect"}));
        std::thread::sleep(std::time::Duration::from_millis(800));
        let _ = lazed::ensure_server();
    });
    Ok(())
}

pub fn run() {
    // One channel slot shared by the daemon event stream and the automation
    // engine — subscribe_events writes the live Channel into it.
    let events_slot: Arc<Mutex<Option<Channel<Value>>>> = Arc::new(Mutex::new(None));
    let events_for_setup = events_slot.clone();
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
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
            control: Mutex::new(HashMap::new()),
            events_running: Mutex::new(false),
            events_channel: events_slot,
        })
        .invoke_handler(tauri::generate_handler![
            bootstrap,
            session_snapshot,
            session_status,
            server_restart,
            term_attach,
            term_input,
            term_resize,
            term_scroll,
            term_detach,
            term_close,
            term_create,
            term_send,
            term_read,
            notify_agent,
            project_create,
            project_focus,
            project_close,
            project_rename,
            group_create,
            group_rename,
            group_remove,
            group_assign,
            workspace_create,
            workspace_remove,
            workspace_rename,
            tab_create,
            tab_close,
            worktree_diff,
            worktree_merge,
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
            open_url,
        ])
        .build(tauri::generate_context!())
        .unwrap_or_else(|err| {
            eprintln!("error while building lazed: {err}");
            std::process::exit(1);
        });
    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            // close attach-stream sockets — the daemon unsubscribes on EOF
            if let Some(state) = handle.try_state::<AppState>() {
                if let Ok(mut map) = state.control.lock() {
                    for (_, h) in map.drain() {
                        let _ = h.stream.shutdown(std::net::Shutdown::Both);
                    }
                }
            }
        }
    });
}
