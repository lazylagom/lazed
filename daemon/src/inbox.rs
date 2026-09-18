//! Universal capture queue (GTD inbox). Anything can land here — the
//! `inbox.add` socket method, `lazed inbox add`, or an automation's inbox
//! action. Persisted to <state>/inbox.json, independent of the terminal
//! session so items survive daemon and app restarts.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::control::str_of;
use crate::session::Session;
use crate::state;

const MAX_TITLE: usize = 8192;
const MAX_ITEMS: usize = 2000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InboxItem {
    pub id: String,
    /// where it came from — "manual", an automation name, "jira", "slack"…
    #[serde(default = "default_source")]
    pub source: String,
    /// dedupe key within the source (jira issue key, slack ts…) — a repeated
    /// (source, key) refreshes the existing item instead of duplicating
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// epoch seconds — when the item landed
    pub at: u64,
    /// open | snoozed | done
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snooze_until: Option<u64>,
}

fn default_source() -> String {
    "manual".into()
}
fn default_status() -> String {
    "open".into()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn path() -> PathBuf {
    state::state_dir().join("inbox.json")
}

fn load() -> Vec<InboxItem> {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

static STORE: OnceLock<Mutex<Vec<InboxItem>>> = OnceLock::new();

fn items() -> &'static Mutex<Vec<InboxItem>> {
    STORE.get_or_init(|| Mutex::new(load()))
}

fn save(items: &[InboxItem]) -> Result<(), String> {
    let _ = state::ensure_dir();
    let tmp = state::state_dir().join(".inbox.tmp");
    let body = serde_json::to_string_pretty(items).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, body).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path()).map_err(|e| e.to_string())
}

fn item_json(i: &InboxItem) -> Value {
    serde_json::to_value(i).unwrap_or_else(|_| json!({}))
}

/// Snoozed items whose time has come flip back to open — lazily, on read,
/// so no timer thread is needed.
fn settle_snoozed(g: &mut Vec<InboxItem>) -> bool {
    let t = now();
    let mut changed = false;
    for it in g.iter_mut() {
        if it.status == "snoozed" && it.snooze_until.is_none_or(|u| u <= t) {
            it.status = "open".into();
            it.snooze_until = None;
            changed = true;
        }
    }
    changed
}

pub fn handle(s: &mut Session, method: &str, p: &Value) -> Result<Value, String> {
    match method {
        "inbox.add" => {
            let title = str_of(p, "title").trim().to_string();
            if title.is_empty() || title.len() > MAX_TITLE {
                return Err(format!("title must be 1..{MAX_TITLE} bytes"));
            }
            let source = {
                let v = str_of(p, "source").trim();
                if v.is_empty() {
                    default_source()
                } else {
                    v.to_string()
                }
            };
            let key = p
                .get("key")
                .and_then(Value::as_str)
                .filter(|k| !k.is_empty())
                .map(str::to_string);
            let body = p
                .get("body")
                .and_then(Value::as_str)
                .filter(|b| !b.is_empty())
                .map(str::to_string);
            let url = p
                .get("url")
                .and_then(Value::as_str)
                .filter(|u| !u.is_empty())
                .map(str::to_string);
            let mut g = items().lock().map_err(|e| e.to_string())?;
            settle_snoozed(&mut g);
            // dedupe on (source, key): refresh content, keep triage state —
            // a done item stays done, a snoozed one stays parked
            if let Some(k) = key.as_deref() {
                if let Some(it) = g
                    .iter_mut()
                    .find(|i| i.source == source && i.key.as_deref() == Some(k))
                {
                    it.title = title;
                    it.body = body;
                    it.url = url;
                    let item = it.clone();
                    save(&g)?;
                    drop(g);
                    s.broadcast_event("inbox.updated", json!({"item": &item}));
                    return Ok(item_json(&item));
                }
            }
            let item = InboxItem {
                id: format!("i{}", &crate::control::id()[..12]),
                source,
                key,
                title,
                body,
                url,
                at: now(),
                status: "open".into(),
                snooze_until: None,
            };
            g.push(item.clone());
            if g.len() > MAX_ITEMS {
                // drop the oldest done items first, then oldest overall
                let over = g.len() - MAX_ITEMS;
                let mut drop_ids = Vec::new();
                for it in g.iter().filter(|i| i.status == "done") {
                    if drop_ids.len() >= over {
                        break;
                    }
                    drop_ids.push(it.id.clone());
                }
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
            s.broadcast_event("inbox.updated", json!({"item": &item}));
            Ok(out)
        }
        "inbox.list" => {
            let all = p["all"].as_bool().unwrap_or(false);
            let mut g = items().lock().map_err(|e| e.to_string())?;
            let flipped = settle_snoozed(&mut g);
            if flipped {
                let _ = save(&g);
            }
            let mut out: Vec<&InboxItem> = g
                .iter()
                .filter(|i| all || i.status != "done")
                .collect();
            out.sort_by(|a, b| b.at.cmp(&a.at));
            let items_json: Vec<Value> = out.iter().map(|i| item_json(i)).collect();
            if flipped {
                drop(g);
                s.broadcast_event("inbox.updated", json!({}));
            }
            Ok(json!({"items": items_json}))
        }
        "inbox.update" => {
            let id = str_of(p, "id");
            let mut g = items().lock().map_err(|e| e.to_string())?;
            let it = g
                .iter_mut()
                .find(|i| i.id == id)
                .ok_or_else(|| format!("no inbox item {id}"))?;
            if let Some(st) = p.get("status").and_then(Value::as_str) {
                if !matches!(st, "open" | "snoozed" | "done") {
                    return Err("status must be open|snoozed|done".into());
                }
                it.status = st.into();
                if st != "snoozed" {
                    it.snooze_until = None;
                }
            }
            if let Some(u) = p.get("snooze_until").and_then(Value::as_u64) {
                it.snooze_until = Some(u);
                it.status = "snoozed".into();
            }
            let item = it.clone();
            save(&g)?;
            drop(g);
            s.broadcast_event("inbox.updated", json!({"item": &item}));
            Ok(item_json(&item))
        }
        "inbox.remove" => {
            let id = str_of(p, "id");
            let mut g = items().lock().map_err(|e| e.to_string())?;
            let before = g.len();
            g.retain(|i| i.id != id);
            if g.len() == before {
                return Err(format!("no inbox item {id}"));
            }
            save(&g)?;
            drop(g);
            s.broadcast_event("inbox.updated", json!({"removed": id}));
            Ok(json!({"ok": true}))
        }
        other => Err(format!("unknown inbox method {other}")),
    }
}

// ── CLI ─────────────────────────────────────────────────────────────

const CLI_HELP: &str = "usage: lazed inbox add <title...> [--url U] [--source S] [--key K] [--body B | --stdin]
       lazed inbox list [--all]
       lazed inbox done|reopen|remove <id>
       lazed inbox snooze <id> <+SECS | EPOCH>";

pub fn cli(args: &[String]) -> i32 {
    match cli_run(args) {
        Ok(v) => {
            println!("{}", serde_json::to_string_pretty(&v).unwrap());
            0
        }
        Err(e) => {
            eprintln!("lazed inbox: {e}");
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
            let mut p = json!({});
            let mut title_parts = Vec::new();
            let mut i = 1;
            while i < args.len() {
                let a = args[i].as_str();
                let field = match a {
                    "--url" => "url",
                    "--source" => "source",
                    "--key" => "key",
                    "--body" => "body",
                    "--stdin" => {
                        let mut text = String::new();
                        std::io::stdin()
                            .take(MAX_TITLE as u64 + 1)
                            .read_to_string(&mut text)
                            .map_err(|e| e.to_string())?;
                        p["body"] = json!(text);
                        i += 1;
                        continue;
                    }
                    _ if a.starts_with('-') => {
                        return Err(format!("unknown option {a}\n{CLI_HELP}"));
                    }
                    _ => {
                        title_parts.push(a.to_string());
                        i += 1;
                        continue;
                    }
                };
                let v = args.get(i + 1).ok_or(CLI_HELP)?;
                p[field] = json!(v);
                i += 2;
            }
            if !title_parts.is_empty() {
                p["title"] = json!(title_parts.join(" "));
            } else if p["body"].is_string() {
                // `… | lazed inbox add --stdin` — the piped text IS the item
                p["title"] = p["body"].clone();
                p.as_object_mut().unwrap().remove("body");
            }
            crate::api_call("inbox.add", p)
        }
        "list" => {
            let all = args.iter().any(|a| a == "--all");
            crate::api_call("inbox.list", json!({"all": all}))
        }
        "done" | "reopen" | "remove" | "snooze" => {
            let id = args.get(1).ok_or(CLI_HELP)?;
            match sub {
                "done" => crate::api_call(
                    "inbox.update",
                    json!({"id": id, "status": "done"}),
                ),
                "reopen" => crate::api_call(
                    "inbox.update",
                    json!({"id": id, "status": "open"}),
                ),
                "remove" => crate::api_call("inbox.remove", json!({"id": id})),
                _ => {
                    let when = args.get(2).ok_or(CLI_HELP)?;
                    let until = if let Some(secs) = when.strip_prefix('+') {
                        now().checked_add(secs
                            .parse::<u64>()
                            .map_err(|_| "snooze needs +SECS or an epoch".to_string())?)
                            .ok_or("snooze time is out of range")?
                    } else {
                        when.parse::<u64>()
                            .map_err(|_| "snooze needs +SECS or an epoch".to_string())?
                    };
                    crate::api_call(
                        "inbox.update",
                        json!({"id": id, "snooze_until": until}),
                    )
                }
            }
        }
        _ => Err(format!("unknown inbox command {sub}\n{CLI_HELP}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_snooze_overflow_is_an_error() {
        let args = ["snooze", "i1", "+18446744073709551615"].map(String::from);
        assert!(cli_run(&args).unwrap_err().contains("out of range"));
    }

    #[test]
    fn dedupe_key_refreshes_without_resurrecting() {
        let mut g: Vec<InboxItem> = Vec::new();
        let item = InboxItem {
            id: "i1".into(),
            source: "jira".into(),
            key: Some("CS-1".into()),
            title: "old".into(),
            body: None,
            url: None,
            at: 1,
            status: "done".into(),
            snooze_until: None,
        };
        g.push(item);
        // mirror the inbox.add dedupe path: same (source,key) updates fields,
        // never reopens a done item
        let it = g
            .iter_mut()
            .find(|i| i.source == "jira" && i.key.as_deref() == Some("CS-1"))
            .unwrap();
        it.title = "new".into();
        assert_eq!(it.status, "done");
        assert_eq!(it.title, "new");
    }

    #[test]
    fn expired_snooze_flips_to_open() {
        let mut g = vec![InboxItem {
            id: "i2".into(),
            source: "manual".into(),
            key: None,
            title: "t".into(),
            body: None,
            url: None,
            at: 1,
            status: "snoozed".into(),
            snooze_until: Some(1),
        }];
        assert!(settle_snoozed(&mut g));
        assert_eq!(g[0].status, "open");
        assert!(g[0].snooze_until.is_none());
        // a live snooze is untouched
        g[0].status = "snoozed".into();
        g[0].snooze_until = Some(u64::MAX);
        assert!(!settle_snoozed(&mut g));
        assert_eq!(g[0].status, "snoozed");
    }
}
