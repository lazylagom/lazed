//! Connected accounts for automations. Non-secret fields live in
//! `<app_config_dir>/integrations.json`; secrets live in the macOS
//! Keychain under the app identifier. Stored values are injected into
//! spawned shells as env vars — only for keys the environment doesn't
//! already define, so ambient env always wins (dev-tool convention, same
//! as ORCA_*_TOKEN taking precedence in Orca).
//!
//! Each provider holds an ordered list of sites (Orca-style: one provider
//! card, N connected accounts). The first fully-configured site feeds the
//! env injection. The token never touches argv — probes export it into
//! the child's environment so `ps` can't see it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::env;

const KEYCHAIN_SERVICE: &str = "com.lazed.app";
const PROBE_TIMEOUT_SECS: u64 = 15;

/// Provider registry — `fields` are the required non-secret keys for a
/// cloud-kind site; `server_fields` for `kind = "server"` sites (Jira
/// self-hosted drops the email requirement). `env_map` maps field keys
/// (incl. the secret `token`) to the env var a spawned shell sees.
struct Provider {
    id: &'static str,
    fields: &'static [&'static str],
    server_fields: &'static [&'static str],
    env_map: &'static [(&'static str, &'static str)],
}

const PROVIDERS: &[Provider] = &[Provider {
    id: "jira",
    fields: &["base", "email"],
    server_fields: &["base"],
    env_map: &[
        ("base", "JIRA_BASE"),
        ("email", "JIRA_EMAIL"),
        ("username", "JIRA_EMAIL"),
        ("token", "JIRA_API_TOKEN"),
    ],
}];

fn provider(id: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|p| p.id == id)
}

/// One connected account under a provider — `fields` holds non-secret
/// config (`kind`, `label`, `base`, `email`, `username`, …); its token
/// lives in the Keychain under `integration.{provider}.{site}.token`.
#[derive(Serialize, Deserialize, Clone)]
struct Site {
    id: String,
    #[serde(default)]
    fields: Map<String, Value>,
}

#[derive(Default, Serialize, Deserialize)]
struct ProviderSites {
    #[serde(default)]
    sites: Vec<Site>,
}

#[derive(Default, Serialize, Deserialize)]
struct Store {
    /// provider id -> ordered connected sites
    #[serde(default)]
    integrations: HashMap<String, ProviderSites>,
}

static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
static STORE_PATH: OnceLock<PathBuf> = OnceLock::new();
/// Tokens read from the Keychain, cached so spawned shells don't
/// re-prompt on every poll. Keyed by "{provider}/{site}".
static TOKENS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

fn tokens() -> std::sync::MutexGuard<'static, HashMap<String, String>> {
    TOKENS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn cache_key(provider: &str, site: &str) -> String {
    format!("{provider}/{site}")
}

fn token_account(provider: &str, site: &str) -> String {
    format!("integration.{provider}.{site}.token")
}

fn keychain_get(provider: &str, site: &str) -> Option<String> {
    let key = cache_key(provider, site);
    if let Some(t) = tokens().get(&key) {
        return Some(t.clone());
    }
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, &token_account(provider, site)).ok()?;
    let token = entry.get_password().ok().filter(|t| !t.is_empty())?;
    tokens().insert(key, token.clone());
    Some(token)
}

fn keychain_set(provider: &str, site: &str, token: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, &token_account(provider, site))
        .map_err(|e| e.to_string())?;
    entry.set_password(token).map_err(|e| e.to_string())?;
    tokens().insert(cache_key(provider, site), token.to_string());
    Ok(())
}

fn keychain_delete(provider: &str, site: &str) {
    tokens().remove(&cache_key(provider, site));
    if let Ok(entry) =
        keyring::Entry::new(KEYCHAIN_SERVICE, &token_account(provider, site))
    {
        let _ = entry.delete_credential();
    }
}

fn with_store<R>(f: impl FnOnce(&mut Store) -> R) -> Result<R, String> {
    let mutex = STORE.get().ok_or("integrations store not initialized")?;
    let mut g = mutex.lock().map_err(|e| e.to_string())?;
    Ok(f(&mut g))
}

fn save_store(store: &Store) -> Result<(), String> {
    let path = STORE_PATH.get().ok_or("integrations store not initialized")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_string_pretty(store).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, body).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// Called once from setup() — resolves the store path, loads it, and
/// migrates the legacy shape (`provider -> flat fields` + one Keychain
/// token) into a single `default` site per provider.
pub fn init(dir: PathBuf) {
    let _ = STORE_PATH.set(dir.join("integrations.json"));
    let path = STORE_PATH.get().cloned().unwrap_or_default();
    let raw: Option<Value> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());
    let mut store = Store::default();
    let mut migrated: Vec<String> = Vec::new();
    if let Some(map) = raw
        .as_ref()
        .and_then(|v| v.get("integrations"))
        .and_then(Value::as_object)
    {
        for (pid, entry) in map {
            let mut ps = ProviderSites::default();
            if let Some(sites) = entry.get("sites").and_then(Value::as_array) {
                for s in sites {
                    if let Ok(site) = serde_json::from_value::<Site>(s.clone()) {
                        ps.sites.push(site);
                    }
                }
            } else if let Some(fields) = entry.as_object() {
                // legacy flat provider — wrap into one "default" site
                let fields: Map<String, Value> = fields
                    .iter()
                    .filter(|(_, v)| v.is_string())
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                if !fields.is_empty() {
                    ps.sites.push(Site {
                        id: "default".to_string(),
                        fields,
                    });
                    migrated.push(pid.clone());
                }
            }
            store.integrations.insert(pid.clone(), ps);
        }
    }
    let dirty = !migrated.is_empty();
    let _ = STORE.set(Mutex::new(store));
    for pid in migrated {
        let legacy = format!("integration.{pid}.token");
        if let Ok(entry) = keyring::Entry::new(KEYCHAIN_SERVICE, &legacy) {
            if let Ok(t) = entry.get_password() {
                if !t.is_empty() {
                    let _ = keychain_set(&pid, "default", &t);
                }
            }
            let _ = entry.delete_credential();
        }
    }
    if dirty {
        let _ = with_store(|s| save_store(s));
    }
}

/// Required non-secret fields depend on the site's `kind` — a self-hosted
/// Jira needs no email (PAT or user:password instead).
fn required_fields<'a>(p: &'a Provider, fields: &Map<String, Value>) -> &'a [&'static str] {
    match fields.get("kind").and_then(Value::as_str) {
        Some("server") if !p.server_fields.is_empty() => p.server_fields,
        _ => p.fields,
    }
}

/// Env vars a provider contributes, in registry order.
fn provider_env_vars(p: &Provider) -> Vec<String> {
    p.env_map.iter().map(|(_, v)| v.to_string()).collect()
}

fn site_configured(p: &Provider, site: &Site) -> bool {
    required_fields(p, &site.fields).iter().all(|f| {
        site.fields
            .get(*f)
            .and_then(Value::as_str)
            .is_some_and(|v| !v.is_empty())
    }) && keychain_get(p.id, &site.id).is_some()
}

/// Values injected into spawned shells for every configured provider —
/// callers merge with `entry().or_insert()` so ambient env wins. The
/// first configured site wins when a provider has several.
pub fn env_overrides() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Ok(store) = STORE.get().ok_or(()).and_then(|m| m.lock().map_err(|_| ())) else {
        return out;
    };
    for p in PROVIDERS {
        let Some(site) = store
            .integrations
            .get(p.id)
            .and_then(|ps| ps.sites.iter().find(|s| site_configured(p, s)))
        else {
            continue;
        };
        for (field, var) in p.env_map {
            let value = if *field == "token" {
                keychain_get(p.id, &site.id)
            } else {
                site.fields
                    .get(*field)
                    .and_then(Value::as_str)
                    .map(str::to_string)
            };
            if let Some(v) = value.filter(|v| !v.is_empty()) {
                out.push(((*var).to_string(), v));
            }
        }
    }
    out
}

fn site_public(p: &Provider, site: &Site) -> Value {
    let mut obj = Map::new();
    for (k, v) in &site.fields {
        if let Some(s) = v.as_str() {
            obj.insert(k.clone(), json!(s));
        }
    }
    json!({
        "id": site.id,
        "label": site.fields.get("label").and_then(Value::as_str),
        "configured": site_configured(p, site),
        "token_set": keychain_get(p.id, &site.id).is_some(),
        "fields": obj,
    })
}

fn provider_public(p: &Provider, ps: Option<&ProviderSites>) -> Value {
    let sites: Vec<Value> = ps
        .map(|ps| ps.sites.iter().map(|s| site_public(p, s)).collect())
        .unwrap_or_default();
    let configured = ps.is_some_and(|ps| ps.sites.iter().any(|s| site_configured(p, s)));
    json!({
        "provider": p.id,
        "configured": configured,
        "env": provider_env_vars(p),
        "sites": sites,
    })
}

pub fn list() -> Result<Value, String> {
    with_store(|s| {
        json!({
            "integrations": PROVIDERS
                .iter()
                .map(|p| provider_public(p, s.integrations.get(p.id)))
                .collect::<Vec<_>>()
        })
    })
}

fn new_site_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("s{nanos:x}")
}

/// Upsert one site under a provider. `site` absent creates a new entry;
/// `token` absent/null keeps the stored secret, "" clears it, a value
/// replaces it (Keychain, never the json file).
pub fn save(input: &Value) -> Result<Value, String> {
    let pid = input
        .get("provider")
        .and_then(Value::as_str)
        .ok_or("missing provider")?;
    let p = provider(pid).ok_or_else(|| format!("unknown provider {pid}"))?;
    let site_id = input
        .get("site")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(new_site_id);
    let out = with_store(|s| -> Result<Value, String> {
        let entry = s.integrations.entry(p.id.to_string()).or_default();
        let idx = match entry.sites.iter().position(|x| x.id == site_id) {
            Some(i) => i,
            None => {
                entry.sites.push(Site {
                    id: site_id.clone(),
                    fields: Map::new(),
                });
                entry.sites.len() - 1
            }
        };
        let site = &mut entry.sites[idx];
        if let Some(fields) = input.get("fields").and_then(Value::as_object) {
            for (k, v) in fields {
                // the secret never lands in the json file
                if k == "token" {
                    continue;
                }
                if let Some(v) = v.as_str() {
                    let v = if k == "base" {
                        v.trim_end_matches('/').to_string()
                    } else {
                        v.to_string()
                    };
                    if v.is_empty() {
                        site.fields.remove(k);
                    } else {
                        site.fields.insert(k.clone(), json!(v));
                    }
                }
            }
        }
        match input.get("token").and_then(Value::as_str) {
            Some("") => keychain_delete(p.id, &site.id),
            Some(t) => keychain_set(p.id, &site.id, t)?,
            None => {}
        }
        let out = provider_public(p, s.integrations.get(p.id));
        let _ = save_store(s);
        Ok(out)
    })??;
    Ok(out)
}

pub fn delete(provider_id: &str, site_id: &str) -> Result<(), String> {
    let p =
        provider(provider_id).ok_or_else(|| format!("unknown provider {provider_id}"))?;
    keychain_delete(p.id, site_id);
    with_store(|s| {
        if let Some(ps) = s.integrations.get_mut(p.id) {
            ps.sites.retain(|x| x.id != site_id);
        }
        let _ = save_store(s);
    })
}

/// Probe a stored site — forces stored values (not ambient env) so the
/// button validates what the user saved.
pub fn test(provider_id: &str, site_id: &str) -> Result<Value, String> {
    let p =
        provider(provider_id).ok_or_else(|| format!("unknown provider {provider_id}"))?;
    let (fields, token) = {
        let store = STORE
            .get()
            .and_then(|m| m.lock().ok())
            .ok_or("store not initialized")?;
        let site = store
            .integrations
            .get(p.id)
            .and_then(|ps| ps.sites.iter().find(|s| s.id == site_id))
            .ok_or("site not found")?;
        if !site_configured(p, site) {
            return Ok(json!({"ok": false, "error": "not configured"}));
        }
        (
            site.fields.clone(),
            keychain_get(p.id, &site.id).unwrap_or_default(),
        )
    };
    match p.id {
        "jira" => probe_jira(&fields, &token),
        _ => Err(format!("unknown provider {}", p.id)),
    }
}

/// Validate unsaved credentials — the connect modal checks before it
/// saves. `input` = `{provider, fields, token}`.
pub fn probe(input: &Value) -> Result<Value, String> {
    let pid = input
        .get("provider")
        .and_then(Value::as_str)
        .ok_or("missing provider")?;
    let fields = input
        .get("fields")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let token = input
        .get("token")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    match pid {
        "jira" => probe_jira(&fields, &token),
        _ => Err(format!("unknown provider {pid}")),
    }
}

/// Jira: GET {base}/rest/api/{3|2}/myself — basic auth for cloud
/// (email:token) and for server sites with a username; Bearer for
/// server PATs. A follow-up serverInfo call grabs the site title.
fn probe_jira(fields: &Map<String, Value>, token: &str) -> Result<Value, String> {
    let kind = fields
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("cloud");
    let base = fields
        .get("base")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim_end_matches('/')
        .to_string();
    if base.is_empty() {
        return Ok(json!({"ok": false, "error": "missing site URL"}));
    }
    if token.is_empty() {
        return Ok(json!({"ok": false, "error": "missing API token"}));
    }
    let api = if kind == "server" { "2" } else { "3" };
    let user = fields
        .get("email")
        .or_else(|| fields.get("username"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let bearer = kind == "server" && user.is_empty();
    if !bearer && user.is_empty() {
        return Ok(json!({"ok": false, "error": "missing Atlassian email"}));
    }
    // credentials go through the env, never argv — expand via sh
    let script = if bearer {
        "curl -fsS -m 10 -H \"Authorization: Bearer $JIRA_TOKEN\" \"$JIRA_BASE/rest/api/$JIRA_API/myself\""
    } else {
        "curl -fsS -m 10 -u \"$JIRA_USER:$JIRA_TOKEN\" \"$JIRA_BASE/rest/api/$JIRA_API/myself\""
    };
    let myself = match jira_get(script, &base, api, user, token) {
        Ok(v) => v,
        Err(e) => return Ok(e),
    };
    let name = myself
        .get("displayName")
        .and_then(Value::as_str)
        .unwrap_or("connected");
    let mail = myself.get("emailAddress").and_then(Value::as_str);
    let detail = match mail {
        Some(m) => format!("{name} — {m}"),
        None => name.to_string(),
    };
    let title_script = if bearer {
        "curl -fsS -m 10 -H \"Authorization: Bearer $JIRA_TOKEN\" \"$JIRA_BASE/rest/api/$JIRA_API/serverInfo\""
    } else {
        "curl -fsS -m 10 -u \"$JIRA_USER:$JIRA_TOKEN\" \"$JIRA_BASE/rest/api/$JIRA_API/serverInfo\""
    };
    let title = jira_get(title_script, &base, api, user, token)
        .ok()
        .and_then(|v| v.get("serverTitle").and_then(Value::as_str).map(str::to_string));
    match title {
        Some(t) => Ok(json!({"ok": true, "detail": detail, "title": t})),
        None => Ok(json!({"ok": true, "detail": detail})),
    }
}

fn jira_get(
    script: &str,
    base: &str,
    api: &str,
    user: &str,
    token: &str,
) -> Result<Value, Value> {
    let mut sh = Command::new("sh");
    sh.args(["-c", script])
        .env("JIRA_BASE", base)
        .env("JIRA_API", api)
        .env("JIRA_USER", user)
        .env("JIRA_TOKEN", token);
    match env::capture(&mut sh, PROBE_TIMEOUT_SECS) {
        Ok(out) => serde_json::from_str(&out)
            .map_err(|e| json!({"ok": false, "error": format!("bad response: {e}")})),
        Err(e) => Err(json!({"ok": false, "error": e})),
    }
}
