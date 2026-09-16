//! Work automations: poll a shell command on an interval, dedupe its output
//! lines as items, and fire an action for each new item — collect only,
//! macOS notification, shell command, or a lazed agent spawn.
//!
//! State lives in `<app_config_dir>/automations.json`. Pollers and `command`
//! actions always run locally; only the `agent` action talks to the daemon.

use std::collections::HashSet;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::ipc::Channel;
use tauri::Manager;
use tauri_plugin_notification::NotificationExt;

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
    /// JSON lines are understood ({id|key|name, title|summary|text}); plain
    /// lines use the first whitespace token as the item id.
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub action: Action,
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

#[derive(Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    automations: Vec<Automation>,
}

static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
static STORE_PATH: OnceLock<PathBuf> = OnceLock::new();
static APP: OnceLock<tauri::AppHandle> = OnceLock::new();
/// Shared slot for the webview's event channel — the same cell lib.rs's
/// AppState owns, so automation updates ride the existing event stream.
pub type EventSlot = std::sync::Arc<Mutex<Option<Channel<Value>>>>;
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
                let _ = c.send(json!({ "event": name, "type": name, "data": data }));
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
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_string_pretty(store).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, body).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn with_store<R>(f: impl FnOnce(&mut Store) -> R) -> Result<R, String> {
    let mutex = STORE.get().ok_or("automation store not initialized")?;
    let mut g = mutex.lock().map_err(|e| e.to_string())?;
    Ok(f(&mut g))
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
        return AutomationItem {
            id,
            text,
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

/// `sh -c <cmd>` with a deadline. stdout/stderr are drained on threads so a
/// noisy command can't deadlock against a full pipe.
fn run_shell(cmd: &str, timeout_secs: u64) -> Result<String, String> {
    let mut child = Command::new("sh")
        .args(["-c", cmd])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn failed: {e}"))?;
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let out_t = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout
            .as_mut()
            .map(|o| o.read_to_string(&mut s));
        s
    });
    let err_t = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr
            .as_mut()
            .map(|o| o.read_to_string(&mut s));
        s
    });
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {
                if Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = out_t.join();
                    let _ = err_t.join();
                    return Err(format!("timed out after {timeout_secs}s"));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = out_t.join();
                let _ = err_t.join();
                return Err(format!("wait failed: {e}"));
            }
        }
    };
    let out = out_t.join().unwrap_or_default();
    let err = err_t.join().unwrap_or_default();
    if status.success() {
        Ok(out)
    } else {
        let tail: String = err
            .lines()
            .rev()
            .take(3)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join(" | ");
        Err(if tail.is_empty() {
            format!("exit {}", status)
        } else {
            format!("exit {}: {}", status, tail.chars().take(300).collect::<String>())
        })
    }
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

/// `agent` action: terminal in the configured project → agent → prompt.
/// Mirrors the fanout sequencing (settle → detect → prompt).
fn run_agent_action(
    agent_kind: &str,
    project_id: &str,
    prompt_tpl: &str,
    item: &AutomationItem,
) -> Result<(), String> {
    let label = format!("auto-{}", item.id.chars().take(24).collect::<String>());
    let res = lazed::api_call(
        "terminal.create",
        json!({"project_id": project_id, "label": label}),
    )?;
    let term_id = res
        .get("term_id")
        .or_else(|| res.pointer("/terminal/term_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "terminal created but no term_id in response".to_string())?;
    // fresh shell needs a beat before `agent.start`
    std::thread::sleep(Duration::from_millis(1500));
    lazed::api_call(
        "agent.start",
        json!({"term_id": term_id, "kind": agent_kind}),
    )?;
    // the daemon flips agent_status as its detector recognizes the TUI —
    // wait for it to leave "unknown", then a short settle for paint
    let deadline = Instant::now() + Duration::from_secs(6);
    while Instant::now() < deadline {
        if let Ok(a) =
            lazed::api_call("agent.get", json!({"term_id": term_id}))
        {
            let st = a
                .get("agent_status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            if st != "unknown" {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    std::thread::sleep(Duration::from_secs(1));
    lazed::api_call(
        "agent.prompt",
        json!({"term_id": term_id, "text": render(prompt_tpl, item, false)}),
    )?;
    Ok(())
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
    }
}

fn mark_error(id: &str, err: String) {
    let _ = with_store(|s| {
        if let Some(a) = s.automations.iter_mut().find(|a| a.id == id) {
            a.last_error = Some(err);
        }
    });
    if let Some(s) = STORE.get().and_then(|m| m.lock().ok()) {
        let _ = save_store(&s);
    }
    emit("automation.updated", json!({}));
}

/// One poll cycle: collect new items, then run the action for each.
/// `force` (run-now) ignores `enabled` and the interval; seen/seeded still apply.
fn run_automation(id: &str, _force: bool) {
    let cmd = match with_store(|s| {
        s.automations
            .iter()
            .find(|a| a.id == id)
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
    let (to_fire, auto_snapshot) = match with_store(|s| {
        let Some(a) = s.automations.iter_mut().find(|a| a.id == id) else {
            return None;
        };
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
        Some((fire, a.clone()))
    }) {
        Ok(Some(v)) => v,
        _ => return,
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
    let _ = with_store(|s| {
        let _ = save_store(s);
    });
    emit("automation.updated", json!({}));
}

fn tick() {
    let due: Vec<String> = with_store(|s| {
        let now = now_secs();
        let mut due = Vec::new();
        for a in s.automations.iter_mut() {
            if !a.enabled || a.command.trim().is_empty() {
                continue;
            }
            let elapsed = a.last_run_at.map(|t| now.saturating_sub(t));
            if elapsed.map(|e| e >= a.interval_secs).unwrap_or(true) {
                a.last_run_at = Some(now);
                due.push(a.id.clone());
            }
        }
        due
    })
    .unwrap_or_default();
    for id in due {
        run_automation(&id, false);
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
    let out = with_store(|s| -> Result<Value, String> {
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
            let out = public(a);
            let _ = save_store(s);
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
        let _ = save_store(s);
        Ok(out)
    })??;
    emit("automation.updated", json!({}));
    Ok(out)
}

pub fn delete(id: &str) -> Result<(), String> {
    with_store(|s| {
        s.automations.retain(|a| a.id != id);
        let _ = save_store(s);
    })?;
    emit("automation.updated", json!({}));
    Ok(())
}

pub fn set_enabled(id: &str, enabled: bool) -> Result<(), String> {
    with_store(|s| -> Result<(), String> {
        find(s, id)?.enabled = enabled;
        let _ = save_store(s);
        Ok(())
    })??;
    emit("automation.updated", json!({}));
    Ok(())
}

/// Immediate poll on a worker thread — the command path stays responsive.
pub fn run_now(id: String) {
    std::thread::spawn(move || run_automation(&id, true));
}

/// Forget seen ids — the next poll treats current items as new and fires.
pub fn reset_seen(id: &str) -> Result<(), String> {
    with_store(|s| -> Result<(), String> {
        let a = find(s, id)?;
        a.seen.clear();
        a.seeded = false;
        a.last_items.clear();
        let _ = save_store(s);
        Ok(())
    })??;
    emit("automation.updated", json!({}));
    Ok(())
}

/// Manually fire the action for one collected item.
pub fn fire(id: &str, item_id: &str) -> Result<(), String> {
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
            let _ = save_store(s);
        }
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
    fn render_quotes_only_for_shell() {
        let item = AutomationItem {
            id: "A-1".into(),
            text: "it's here".into(),
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
