//! Personal todo list — the user's own quick-capture checklist on the
//! rail. Unlike the GTD inbox (a triage queue fed by automations), todos
//! are owned by the user: add on a whim via the rail input, `todo.add`,
//! or `lazed todo add`; check off when done; clear the finished. Persisted
//! to <state>/todo.json, independent of the terminal session.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::session::Session;
use crate::state;
use crate::tasks::str_of;

const MAX_TITLE: usize = 8192;
const MAX_ITEMS: usize = 2000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub title: String,
    /// epoch seconds — when the item was added
    pub at: u64,
    #[serde(default)]
    pub done: bool,
    /// epoch seconds — when the item was checked off
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done_at: Option<u64>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn path() -> PathBuf {
    state::state_dir().join("todo.json")
}

fn load() -> Vec<TodoItem> {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

static STORE: OnceLock<Mutex<Vec<TodoItem>>> = OnceLock::new();

fn items() -> &'static Mutex<Vec<TodoItem>> {
    STORE.get_or_init(|| Mutex::new(load()))
}

fn save(items: &[TodoItem]) -> Result<(), String> {
    let _ = state::ensure_dir();
    let tmp = state::state_dir().join(".todo.tmp");
    let body = serde_json::to_string_pretty(items).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, body).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path()).map_err(|e| e.to_string())
}

fn item_json(i: &TodoItem) -> Value {
    serde_json::to_value(i).unwrap_or_else(|_| json!({}))
}

/// Open items first (newest at the top), then done (most recently checked).
fn sorted(g: &[TodoItem]) -> Vec<&TodoItem> {
    let mut out: Vec<&TodoItem> = g.iter().collect();
    out.sort_by(|a, b| {
        a.done
            .cmp(&b.done)
            .then(if a.done {
                b.done_at.unwrap_or(b.at).cmp(&a.done_at.unwrap_or(a.at))
            } else {
                b.at.cmp(&a.at)
            })
    });
    out
}

pub fn handle(s: &mut Session, method: &str, p: &Value) -> Result<Value, String> {
    match method {
        "todo.add" => {
            let title = str_of(p, "title").trim().to_string();
            if title.is_empty() || title.len() > MAX_TITLE {
                return Err(format!("title must be 1..{MAX_TITLE} bytes"));
            }
            let mut g = items().lock().map_err(|e| e.to_string())?;
            let item = TodoItem {
                id: format!("d{}", &crate::tasks::random_id()[..12]),
                title,
                at: now(),
                done: false,
                done_at: None,
            };
            g.push(item.clone());
            if g.len() > MAX_ITEMS {
                // drop the oldest done items first, then oldest overall
                let over = g.len() - MAX_ITEMS;
                let mut drop_ids: Vec<String> =
                    g.iter().filter(|i| i.done).map(|i| i.id.clone()).collect();
                drop_ids.truncate(over);
                for it in g.iter() {
                    if drop_ids.len() >= over {
                        break;
                    }
                    if !drop_ids.contains(&it.id) {
                        drop_ids.push(it.id.clone());
                    }
                }
                g.retain(|i| !drop_ids.contains(&i.id));
            }
            save(&g)?;
            let out = item_json(&item);
            drop(g);
            s.broadcast_event("todo.updated", json!({"item": &item}));
            Ok(out)
        }
        "todo.list" => {
            let g = items().lock().map_err(|e| e.to_string())?;
            let items_json: Vec<Value> = sorted(&g).iter().map(|i| item_json(i)).collect();
            Ok(json!({"items": items_json}))
        }
        "todo.update" => {
            let id = str_of(p, "id");
            let mut g = items().lock().map_err(|e| e.to_string())?;
            let it = g
                .iter_mut()
                .find(|i| i.id == id)
                .ok_or_else(|| format!("no todo item {id}"))?;
            if let Some(title) = p.get("title").and_then(Value::as_str) {
                let title = title.trim();
                if title.is_empty() || title.len() > MAX_TITLE {
                    return Err(format!("title must be 1..{MAX_TITLE} bytes"));
                }
                it.title = title.into();
            }
            if let Some(done) = p.get("done").and_then(Value::as_bool) {
                it.done = done;
                it.done_at = done.then(now);
            }
            let item = it.clone();
            save(&g)?;
            drop(g);
            s.broadcast_event("todo.updated", json!({"item": &item}));
            Ok(item_json(&item))
        }
        "todo.remove" => {
            let id = str_of(p, "id");
            let mut g = items().lock().map_err(|e| e.to_string())?;
            let before = g.len();
            g.retain(|i| i.id != id);
            if g.len() == before {
                return Err(format!("no todo item {id}"));
            }
            save(&g)?;
            drop(g);
            s.broadcast_event("todo.updated", json!({"removed": id}));
            Ok(json!({"ok": true}))
        }
        "todo.clear" => {
            let mut g = items().lock().map_err(|e| e.to_string())?;
            let before = g.len();
            g.retain(|i| !i.done);
            let removed = before - g.len();
            if removed == 0 {
                return Ok(json!({"cleared": 0}));
            }
            save(&g)?;
            drop(g);
            s.broadcast_event("todo.updated", json!({}));
            Ok(json!({"cleared": removed}))
        }
        other => Err(format!("unknown todo method {other}")),
    }
}

// ── CLI ─────────────────────────────────────────────────────────────

const CLI_HELP: &str = "usage: lazed todo add <title...>
       lazed todo list
       lazed todo done|undone|remove <id>
       lazed todo rename <id> <title...>
       lazed todo clear";

pub fn cli(args: &[String]) -> i32 {
    match cli_run(args) {
        Ok(v) => {
            println!("{}", serde_json::to_string_pretty(&v).unwrap());
            0
        }
        Err(e) => {
            eprintln!("lazed todo: {e}");
            1
        }
    }
}

fn cli_run(args: &[String]) -> Result<Value, String> {
    let sub = args.first().map(String::as_str).unwrap_or("--help");
    if matches!(sub, "--help" | "-h") {
        return Ok(json!({"usage": CLI_HELP}));
    }
    match sub {
        "add" => {
            let title = args[1..].join(" ");
            if title.trim().is_empty() {
                return Err(CLI_HELP.into());
            }
            crate::api_call("todo.add", json!({"title": title}))
        }
        "list" => crate::api_call("todo.list", json!({})),
        "done" | "undone" | "remove" => {
            let id = args.get(1).ok_or(CLI_HELP)?;
            match sub {
                "remove" => crate::api_call("todo.remove", json!({"id": id})),
                _ => crate::api_call(
                    "todo.update",
                    json!({"id": id, "done": sub == "done"}),
                ),
            }
        }
        "rename" => {
            let id = args.get(1).ok_or(CLI_HELP)?;
            let title = args[2..].join(" ");
            if title.trim().is_empty() {
                return Err(CLI_HELP.into());
            }
            crate::api_call("todo.update", json!({"id": id, "title": title}))
        }
        "clear" => crate::api_call("todo.clear", json!({})),
        _ => Err(format!("unknown todo command {sub}\n{CLI_HELP}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, at: u64, done: bool, done_at: Option<u64>) -> TodoItem {
        TodoItem {
            id: id.into(),
            title: id.into(),
            at,
            done,
            done_at,
        }
    }

    #[test]
    fn sorted_lists_open_newest_first_then_done() {
        let g = vec![
            item("d1", 1, true, Some(10)),
            item("d2", 3, false, None),
            item("d3", 2, false, None),
            item("d4", 4, true, Some(20)),
        ];
        let ids: Vec<&str> = sorted(&g).iter().map(|i| i.id.as_str()).collect();
        // open: newest first (d3 < d2 by recency → d2,d3); done: most
        // recently checked first (d4 done_at 20 > d1 done_at 10)
        assert_eq!(ids, ["d2", "d3", "d4", "d1"]);
    }

    #[test]
    fn mixed_done_and_open_compare_by_done_at() {
        // a done item and an open item never compare on `at` — open always
        // wins regardless of timestamps
        let g = vec![item("d1", 100, true, Some(1)), item("d2", 1, false, None)];
        let ids: Vec<&str> = sorted(&g).iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["d2", "d1"]);
    }
}
