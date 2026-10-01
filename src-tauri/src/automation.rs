//! Work automations: poll a shell command on an interval, dedupe its output
//! lines as items, and fire an action for each new item — collect only,
//! macOS notification, shell command, or a lazed agent spawn.
//!
//! State lives in `<app_config_dir>/automations.json`. Pollers and `command`
//! actions always run locally; only the `agent` action talks to the daemon.

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use serde_json::{json, Value};
use tauri::ipc::Channel;
use tauri::Manager;
use tauri_plugin_notification::NotificationExt;

use crate::env;
use crate::lazed;

const TICK_SECS: u64 = 10;
const POLL_TIMEOUT_SECS: u64 = 45;
const ACTION_TIMEOUT_SECS: u64 = 90;
const MIN_INTERVAL_SECS: u64 = 15;
const MAX_ITEMS_PER_RUN: usize = 200;
const MAX_KEPT_ITEMS: usize = 50;
const MAX_SEEN: usize = 2000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AutomationItem {
    pub id: String,
    pub text: String,
    /// source link — picked from `url`/`link`/`permalink` in JSON lines, or
    /// the first http(s) token in plain lines
    #[serde(default)]
    pub url: Option<String>,
    /// provider slug ("jira", "slack", "gmail"…) — picked from `provider` in
    /// JSON lines; the inbox action groups by it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// epoch seconds when the item was first seen
    pub at: u64,
    /// an action run completed for this item (auto or manual)
    #[serde(default)]
    pub fired: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    /// Just collect items into the panel — fire manually per item.
    Collect,
    /// macOS notification per new item.
    Notify,
    /// Run a shell command template; {id}/{text} are shell-quoted.
    Command { command: String },
    /// New terminal in `project_id` → agent start → prompt template.
    Agent {
        agent_kind: String,
        project_id: String,
        prompt: String,
    },
    /// Drop the item into the daemon inbox for GTD triage.
    Inbox,
}

impl Default for Action {
    fn default() -> Self {
        Action::Collect
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Automation {
    pub id: String,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_interval")]
    pub interval_secs: u64,
    /// Poller: a shell command whose stdout yields one item per line.
    /// JSON lines are understood ({id|key|name, title|summary|text,
    /// url|link|permalink, provider}); plain lines use the first whitespace
    /// token as the item id.
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub action: Action,
    /// Catalog preset this automation was switched on from (e.g.
    /// `jira-mention`) — lets the Automations screen map the ready-made
    /// list onto stored automations even after the command is edited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// Ids already recorded — never fired twice.
    #[serde(default)]
    pub seen: HashSet<String>,
    /// False until the first successful poll; that first poll records items
    /// without firing so creating a watcher doesn't fire a backlog.
    #[serde(default)]
    pub seeded: bool,
    #[serde(default)]
    pub last_run_at: Option<u64>,
    #[serde(default)]
    pub last_ok_at: Option<u64>,
    #[serde(default)]
    pub last_error: Option<String>,
    /// Newest-first recent items for the panel.
    #[serde(default)]
    pub last_items: Vec<AutomationItem>,
    #[serde(default)]
    pub fire_count: u64,
}

fn default_true() -> bool {
    true
}
fn default_interval() -> u64 {
    300
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    automations: Vec<Automation>,
}

static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
static STORE_PATH: OnceLock<PathBuf> = OnceLock::new();
static APP: OnceLock<tauri::AppHandle> = OnceLock::new();
/// Shared slot for the webview's event channel — the same cell lib.rs's
/// AppState owns, so automation updates ride the existing event stream.
pub type EventSlot = std::sync::Arc<Mutex<Option<Channel<Box<RawValue>>>>>;
static EVENTS: OnceLock<EventSlot> = OnceLock::new();

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn emit(name: &str, data: Value) {
    if let Some(slot) = EVENTS.get() {
        if let Ok(g) = slot.lock() {
            if let Some(c) = g.as_ref() {
                let _ = c.send(crate::raw_json(&json!({ "event": name, "type": name, "data": data })));
            }
        }
    }
}

fn load_store(path: &PathBuf) -> Store {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_store(store: &Store) -> Result<(), String> {
    let path = STORE_PATH.get().ok_or("automation store not initialized")?;
    let body = serde_json::to_vec_pretty(store).map_err(|e| e.to_string())?;
    crate::state_file::write(path, &body)

}

fn with_store<R>(f: impl FnOnce(&mut Store) -> R) -> Result<R, String> {
    let mutex = STORE.get().ok_or("automation store not initialized")?;
    let mut g = mutex.lock().map_err(|e| e.to_string())?;
    Ok(f(&mut g))
}

fn commit_with<R>(store: &mut Store, edit: impl FnOnce(&mut Store) -> Result<R, String>, write: impl FnOnce(&Store) -> Result<(), String>) -> Result<R, String> {
    let mut candidate = store.clone();
    let result = edit(&mut candidate)?;
    write(&candidate)?;
    *store = candidate;
    Ok(result)
}

fn edit_store<R>(edit: impl FnOnce(&mut Store) -> Result<R, String>) -> Result<R, String> {
    with_store(|store| commit_with(store, edit, save_store))?
}

static RUNS: OnceLock<std::sync::Arc<crate::automation_gate::Gate>> = OnceLock::new();
fn run_permit(id: &str) -> Result<crate::automation_gate::Permit, String> {
    RUNS.get_or_init(Default::default).acquire(id)
}

/// An automation reduced to its public shape — `seen` is internal state and
/// can be thousands of ids, so the API reports only its size.
fn public(a: &Automation) -> Value {
    let mut v = serde_json::to_value(a).unwrap_or_else(|_| json!({}));
    if let Some(obj) = v.as_object_mut() {
        obj.insert("seen_count".to_string(), json!(a.seen.len()));
        obj.remove("seen");
    }
    v
}

/// First http(s) token in a plain line, if any.
fn find_url(line: &str) -> Option<String> {
    line.split_whitespace()
        .find(|t| t.starts_with("https://") || t.starts_with("http://"))
        .map(str::to_string)
}

/// Parse one stdout line into an item: JSON objects give structured ids,
/// anything else falls back to `first-token / whole-line`.
fn parse_item(line: &str) -> AutomationItem {
    if let Ok(v) = serde_json::from_str::<Value>(line) {
        let pick = |keys: &[&str]| {
            keys.iter()
                .find_map(|k| v.get(*k).and_then(Value::as_str).map(str::to_string))
        };
        let id = pick(&["id", "key", "name", "issue"])
            .unwrap_or_else(|| line.chars().take(64).collect());
        let text = pick(&["title", "summary", "text", "description"])
            .map(|t| format!("{id} {t}"))
            .unwrap_or_else(|| line.to_string());
        let url = pick(&["url", "link", "permalink"])
            .filter(|u| !u.is_empty())
            .or_else(|| find_url(line));
        let provider = pick(&["provider"])
            .map(|p| p.trim().to_lowercase())
            .filter(|p| !p.is_empty());
        return AutomationItem {
            id,
            text,
            url,
            provider,
            at: now_secs(),
            fired: false,
        };
    }
    let id = line
        .split_whitespace()
        .next()
        .unwrap_or(line)
        .to_string();
    AutomationItem {
        id,
        text: line.to_string(),
        url: find_url(line),
        provider: None,
        at: now_secs(),
        fired: false,
    }
}

fn parse_items(stdout: &str) -> Vec<AutomationItem> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let item = parse_item(line);
        if seen.insert(item.id.clone()) {
            out.push(item);
        }
        if out.len() >= MAX_ITEMS_PER_RUN {
            break;
        }
    }
    out
}

/// `sh -c <cmd>` with a deadline, under the resolved spawn env (login
/// PATH + fallback dirs + integration values).
fn run_shell(cmd: &str, timeout_secs: u64) -> Result<String, String> {
    let mut c = Command::new("sh");
    c.args(["-c", cmd]).envs(env::env_for_spawn());
    env::capture(&mut c, timeout_secs)
}

/// POSIX single-quote escaping for command-action templates.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn render(tpl: &str, item: &AutomationItem, quote: bool) -> String {
    let (id, text) = if quote {
        (shell_quote(&item.id), shell_quote(&item.text))
    } else {
        (item.id.clone(), item.text.clone())
    };
    tpl.replace("{id}", &id)
        .replace("{text}", &text)
        .replace("{title}", &text)
}

/// `agent` action: a fresh tab in the project's main workspace → herdr
/// agent → prompt. Readiness and submission are herdr's (agent.start waits
/// for the interactive prompt; agent.prompt refuses a blocked agent).
fn run_agent_action(
    agent_kind: &str,
    project_id: &str,
    prompt_tpl: &str,
    item: &AutomationItem,
) -> Result<(), String> {
    let snap = lazed::api_call("session.snapshot", json!({}))?;
    let ws = snap
        .get("workspaces")
        .and_then(Value::as_array)
        .and_then(|ws| {
            ws.iter().find(|w| {
                w.get("project_id").and_then(Value::as_str) == Some(project_id)
                    && w.get("is_main").and_then(Value::as_bool) == Some(true)
            })
        })
        .and_then(|w| w.get("workspace_id").and_then(Value::as_str))
        .ok_or_else(|| format!("project {project_id} has no open main workspace (herdr down or project closed)"))?
        .to_string();
    let tab = lazed::herdr_call("tab.create", json!({"workspace_id": ws}))?;
    let pane_id = tab
        .pointer("/root_pane/pane_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "tab created but no root pane in response".to_string())?;
    let name = format!(
        "auto-{}",
        item.id.to_ascii_lowercase().chars()
            .map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' { c } else { '-' })
            .take(26)
            .collect::<String>()
    );
    lazed::herdr_call(
        "agent.start",
        json!({"name": name, "kind": agent_kind, "pane_id": pane_id}),
    )?;
    lazed::herdr_call(
        "agent.prompt",
        json!({"target": pane_id, "text": render(prompt_tpl, item, false)}),
    )?;
    Ok(())
}

/// Provider slug for a catalog preset — its first '-' segment ("jira-rest" →
/// "jira"), with aliases for ids that don't name their provider
/// ("gh-review" → "github"). Items without one group under the automation
/// name in the inbox.
fn preset_provider(preset: &str) -> Option<String> {
    let head = preset.split('-').next()?.trim().to_lowercase();
    match head.as_str() {
        "" | "custom" => None,
        "gh" => Some("github".into()),
        h => Some(h.to_string()),
    }
}

fn run_action(auto: &Automation, item: &AutomationItem) -> Result<(), String> {
    match &auto.action {
        Action::Collect => Ok(()),
        Action::Notify => {
            let app = APP.get().ok_or("app handle unavailable")?;
            app.notification()
                .builder()
                .title(&auto.name)
                .body(&item.text)
                .show()
                .map_err(|e| e.to_string())
        }
        Action::Command { command } => {
            run_shell(&render(command, item, true), ACTION_TIMEOUT_SECS).map(|_| ())
        }
        Action::Agent {
            agent_kind,
            project_id,
            prompt,
        } => run_agent_action(agent_kind, project_id, prompt, item),
        Action::Inbox => lazed::api_call(
            "inbox.add",
            json!({
                "title": item.text,
                "key": item.id,
                "source": auto.name,
                "provider": item.provider.clone().or_else(|| {
                    auto.preset.as_deref().and_then(preset_provider)
                }),
                "url": item.url,
            }),
        )
        .map(|_| ()),
    }
}

fn mark_error(id: &str, err: String) {
    let _ = with_store(|s| {
        if let Some(a) = s.automations.iter_mut().find(|a| a.id == id) {
            a.last_error = Some(err);
        }
    });
    if let Some(s) = STORE.get().and_then(|m| m.lock().ok()) {
        if let Err(error) = save_store(&s) { eprintln!("[lazed] automation error state could not be persisted: {error}"); }
    }
    emit("automation.updated", json!({}));
}

/// One poll cycle: collect new items, then run the action for each.
/// `force` (run-now) ignores `enabled` and the interval; seen/seeded still apply.
fn run_automation(id: &str, force: bool) {
    let cmd = match with_store(|s| {
        s.automations
            .iter()
            .find(|a| a.id == id)
            .filter(|a| force || a.enabled)
            .map(|a| a.command.clone())
    }) {
        Ok(Some(v)) => v,
        _ => return,
    };
    let items = match run_shell(&cmd, POLL_TIMEOUT_SECS) {
        Ok(out) => parse_items(&out),
        Err(e) => {
            mark_error(id, e);
            return;
        }
    };
    // Merge results under the lock: unseen ids → fire list. First successful
    // poll only seeds — everything it returns is recorded, nothing fires.
    let (to_fire, auto_snapshot) = match edit_store(|s| {
        let Some(a) = s.automations.iter_mut().find(|a| a.id == id) else {
            return Ok(None);
        };
        if a.command != cmd || (!force && !a.enabled) { return Ok(None); }
        a.last_ok_at = Some(now_secs());
        a.last_error = None;
        let mut fire = Vec::new();
        if a.seeded {
            for item in items {
                if a.seen.insert(item.id.clone()) {
                    fire.push(item.clone());
                    a.last_items.insert(0, item);
                }
            }
        } else {
            a.seeded = true;
            for item in items {
                a.seen.insert(item.id.clone());
                a.last_items.insert(0, item);
            }
        }
        if a.seen.len() > MAX_SEEN {
            let keep: HashSet<String> =
                a.last_items.iter().map(|i| i.id.clone()).collect();
            a.seen.retain(|i| keep.contains(i));
        }
        a.last_items.truncate(MAX_KEPT_ITEMS);
        Ok(Some((fire, a.clone())))
    }) {
        Ok(Some(v)) => v,
        Ok(None) => return,
        Err(error) => { mark_error(id, error); return; }
    };
    for item in to_fire {
        let result = run_action(&auto_snapshot, &item);
        let _ = with_store(|s| {
            if let Some(a) = s.automations.iter_mut().find(|a| a.id == id) {
                match &result {
                    Ok(()) => {
                        a.fire_count += 1;
                        if let Some(i) =
                            a.last_items.iter_mut().find(|i| i.id == item.id)
                        {
                            i.fired = true;
                        }
                    }
                    Err(e) => a.last_error = Some(e.clone()),
                }
            }
        });
        if let Err(e) = &result {
            eprintln!("[lazed] automation {id} action failed: {e}");
        }
    }
    if let Err(error) = with_store(|s| save_store(s)).and_then(|result| result) {
        mark_error(id, format!("action state persistence failed: {error}; inspect before retrying"));
    }
    emit("automation.updated", json!({}));
}

fn tick() {
    let due: Vec<String> = with_store(|s| {
        let now = now_secs();
        s.automations.iter().filter(|a| a.enabled && !a.command.trim().is_empty()
            && a.last_run_at.map(|t| now.saturating_sub(t) >= a.interval_secs).unwrap_or(true))
            .map(|a| a.id.clone()).collect()
    }).unwrap_or_default();
    for id in due {
        if let Ok(permit) = run_permit(&id) {
            let _ = with_store(|s| { if let Ok(a) = find(s, &id) { a.last_run_at = Some(now_secs()); } });
            std::thread::spawn(move || { let _permit = permit; run_automation(&id, false); });
        }
    }
}

/// Called once from setup(): resolves the store path and starts the
/// scheduler thread. `events` is the same channel slot subscribe_events
/// writes to — automation updates ride that stream.
pub fn init(app: tauri::AppHandle, events: EventSlot) {
    let dir = app
        .path()
        .app_config_dir()
        .unwrap_or_else(|_| PathBuf::from("."));
    let _ = STORE_PATH.set(dir.join("automations.json"));
    let _ = APP.set(app);
    let _ = EVENTS.set(events);
    let path = STORE_PATH.get().cloned().unwrap_or_default();
    let _ = STORE.set(Mutex::new(load_store(&path)));
    std::thread::spawn(|| loop {
        tick();
        std::thread::sleep(Duration::from_secs(TICK_SECS));
    });
}

fn find<'a>(s: &'a mut Store, id: &str) -> Result<&'a mut Automation, String> {
    s.automations
        .iter_mut()
        .find(|a| a.id == id)
        .ok_or_else(|| format!("no automation {id}"))
}

pub fn list() -> Result<Value, String> {
    with_store(|s| {
        json!({ "automations": s.automations.iter().map(public).collect::<Vec<_>>() })
    })
}

/// Upsert: empty/absent id mints a new automation; existing id merges the
/// editable fields and preserves run state.
pub fn save(input: Value) -> Result<Value, String> {
    let out = edit_store(|s| -> Result<Value, String> {
        let id = input
            .get("id")
            .and_then(Value::as_str)
            .filter(|i| !i.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("a{:x}", now_secs() * 1000 + s.automations.len() as u64));
        let action = input
            .get("action")
            .cloned()
            .map(|a| serde_json::from_value::<Action>(a))
            .transpose()
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
        let interval = input
            .get("interval_secs")
            .and_then(Value::as_u64)
            .unwrap_or_else(default_interval)
            .max(MIN_INTERVAL_SECS);
        if let Ok(idx) = s
            .automations
            .iter()
            .position(|a| a.id == id)
            .ok_or("missing")
        {
            let a = &mut s.automations[idx];
            a.name = input
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(&a.name)
                .to_string();
            a.command = input
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or(&a.command)
                .to_string();
            a.enabled = input
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(a.enabled);
            a.interval_secs = interval;
            a.action = action;
            if let Some(p) = input.get("preset") {
                a.preset = p.as_str().filter(|p| !p.is_empty()).map(str::to_string);
            }
            let out = public(a);
            return Ok(out);
        }
        let a = Automation {
            id: id.clone(),
            name: input
                .get("name")
                .and_then(Value::as_str)
                .filter(|n| !n.is_empty())
                .unwrap_or("automation")
                .to_string(),
            enabled: input
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            interval_secs: interval,
            command: input
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            action,
            preset: input
                .get("preset")
                .and_then(Value::as_str)
                .filter(|p| !p.is_empty())
                .map(str::to_string),
            seen: HashSet::new(),
            seeded: false,
            last_run_at: None,
            last_ok_at: None,
            last_error: None,
            last_items: Vec::new(),
            fire_count: 0,
        };
        s.automations.push(a);
        let out = public(s.automations.last().unwrap());
        Ok(out)
    })?;
    emit("automation.updated", json!({}));
    Ok(out)
}

pub fn delete(id: &str) -> Result<(), String> {
    edit_store(|s| {
        s.automations.retain(|a| a.id != id);
        Ok(())
    })?;
    emit("automation.updated", json!({}));
    Ok(())
}

pub fn set_enabled(id: &str, enabled: bool) -> Result<(), String> {
    edit_store(|s| -> Result<(), String> {
        find(s, id)?.enabled = enabled;
        Ok(())
    })?;
    emit("automation.updated", json!({}));
    Ok(())
}

/// Immediate poll on a worker thread — the command path stays responsive.
pub fn run_now(id: String) -> Result<(), String> {
    let permit = run_permit(&id)?;
    with_store(|s| { find(s, &id)?.last_run_at = Some(now_secs()); Ok::<_, String>(()) })??;
    std::thread::spawn(move || { let _permit = permit; run_automation(&id, true); });
    Ok(())
}

/// Forget seen ids — the next poll treats current items as new and fires.
pub fn reset_seen(id: &str) -> Result<(), String> {
    edit_store(|s| -> Result<(), String> {
        let a = find(s, id)?;
        a.seen.clear();
        a.seeded = false;
        a.last_items.clear();
        Ok(())
    })?;
    emit("automation.updated", json!({}));
    Ok(())
}

/// Manually fire the action for one collected item.
pub fn fire(id: &str, item_id: &str) -> Result<(), String> {
    let _permit = run_permit(id)?;
    let (auto, item) = with_store(|s| -> Result<(Automation, AutomationItem), String> {
        let a = find(s, id)?;
        let item = a
            .last_items
            .iter()
            .find(|i| i.id == item_id)
            .cloned()
            .ok_or_else(|| format!("no item {item_id}"))?;
        Ok((a.clone(), item))
    })??;
    run_action(&auto, &item)?;
    with_store(|s| {
        if let Ok(a) = find(s, id) {
            a.fire_count += 1;
            if let Some(i) = a.last_items.iter_mut().find(|i| i.id == item_id) {
                i.fired = true;
            }
        }
        save_store(s)
    })?.map_err(|error| {
        let message = format!("action completed; persistence failed: {error}; inspect before retrying");
        mark_error(id, message.clone());
        message
    })?;
    emit("automation.updated", json!({}));
    Ok(())
}

/// Dry-run a poller command for the editor's Test button.
pub fn test(command: &str) -> Result<Value, String> {
    match run_shell(command, POLL_TIMEOUT_SECS) {
        Ok(out) => {
            let items = parse_items(&out);
            Ok(json!({
                "ok": true,
                "items": items.iter().map(|i| json!({"id": i.id, "text": i.text})).collect::<Vec<_>>(),
            }))
        }
        Err(e) => Ok(json!({ "ok": false, "error": e })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_lines_use_first_token_as_id() {
        let items = parse_items("PROJ-12  fix the thing\nPROJ-9\tother bug\n");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "PROJ-12");
        assert_eq!(items[0].text, "PROJ-12  fix the thing");
    }

    #[test]
    fn json_lines_pick_id_and_title() {
        let items =
            parse_items("{\"key\":\"ABC-1\",\"summary\":\"hello\"}\n");
        assert_eq!(items[0].id, "ABC-1");
        assert_eq!(items[0].text, "ABC-1 hello");
    }

    #[test]
    fn duplicate_ids_collapse_and_blanks_skip() {
        let items = parse_items("\nA-1 x\nA-1 again\n  \nB-2 y\n");
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn json_lines_pick_provider() {
        let items = parse_items(
            "{\"id\":\"m1\",\"title\":\"hello\",\"provider\":\" Gmail \"}\nplain line\n",
        );
        assert_eq!(items[0].provider.as_deref(), Some("gmail"));
        assert_eq!(items[1].provider, None);
    }

    #[test]
    fn preset_provider_uses_first_segment_with_aliases() {
        assert_eq!(preset_provider("jira-rest").as_deref(), Some("jira"));
        assert_eq!(preset_provider("jira-mention").as_deref(), Some("jira"));
        assert_eq!(preset_provider("slack-channel").as_deref(), Some("slack"));
        assert_eq!(preset_provider("gh-review").as_deref(), Some("github"));
        assert_eq!(preset_provider("custom"), None);
        assert_eq!(preset_provider(""), None);
    }

    #[test]
    fn render_quotes_only_for_shell() {
        let item = AutomationItem {
            id: "A-1".into(),
            text: "it's here".into(),
            url: None,
            provider: None,
            at: 0,
            fired: false,
        };
        assert_eq!(render("open {id}", &item, true), "open 'A-1'");
        assert_eq!(
            render("fix {text}", &item, true),
            "fix 'it'\\''s here'"
        );
        assert_eq!(render("{id}: {text}", &item, false), "A-1: it's here");
    }
}

#[cfg(test)]
mod persistence_regressions {
    use super::*;
    #[test]
    fn failed_save_retains_previous_configuration_and_invalid_edits_do_not_write() {
        let mut store = Store::default();
        let result = commit_with(&mut store, |candidate| {
            candidate.automations = serde_json::from_value(json!([{"id":"a1", "name":"candidate"}])).unwrap();
            Ok(())
        }, |_| Err("disk full".into()));
        assert_eq!(result.unwrap_err(), "disk full");
        assert!(store.automations.is_empty());
        let result = commit_with(&mut store, |_| Err::<(), String>("invalid configuration".into()), |_| panic!("invalid edit must not write"));
        assert!(result.is_err());
        assert!(store.automations.is_empty());
    }
    #[test]
    fn failed_atomic_replace_leaves_the_previous_file_readable() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 { return; }
        let fixture = crate::repo::tests::fixture();
        let directory = fixture.root.join("private-state");
        std::fs::create_dir(&directory).unwrap();
        let target = directory.join("state.json");
        crate::state_file::write(&target, b"previous").unwrap();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o500)).unwrap();
        let result = crate::state_file::write(&target, b"candidate");
        let previous = std::fs::read(&target);
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(result.is_err());
        assert_eq!(previous.unwrap(), b"previous");
    }
}
