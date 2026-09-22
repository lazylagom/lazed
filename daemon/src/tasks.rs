//! Durable execution records. A task is not a claim of verified code completion.
//!
//! A task = one worktree + one herdr agent + one initial prompt, filed under
//! a caller-chosen `request_id` so a disconnected client can query status
//! instead of blindly resending. herdr owns the agent (start, readiness,
//! lifecycle, names); this module owns the record and the ordering of side
//! effects around it.
use crate::{
    herdr,
    server::Shared,
    session::{lock, Session},
    specs::AgentRegistry,
};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    io::{Read, Write},
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

static ACTIVE: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
static BOOT: OnceLock<String> = OnceLock::new();
fn boot() -> &'static str {
    BOOT.get_or_init(random_id)
}
pub fn random_id() -> String {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .unwrap()
        .read_exact(&mut bytes)
        .unwrap();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub fn str_of<'a>(p: &'a Value, key: &str) -> &'a str {
    p.get(key).and_then(Value::as_str).unwrap_or("")
}
pub fn validate_text(text: &str) -> Result<String, String> {
    let text = text.replace("\r\n", "\n");
    if text.trim().is_empty() || text.len() > 65536 {
        return Err("prompt must contain 1..65536 bytes".into());
    }
    if text
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err("prompt contains terminal control characters".into());
    }
    Ok(text)
}
fn directory() -> PathBuf {
    crate::state::state_dir().join("tasks")
}
fn key(id: &str) -> Result<&str, String> {
    if id.is_empty()
        || id.len() > 100
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
    {
        return Err("request/task ID must be 1..100 ASCII letters, digits, - or _".into());
    }
    Ok(id)
}
/// The herdr agent name for a task — herdr names match
/// `[a-z][a-z0-9_-]{0,31}` and are unique among live agents, so the task
/// id (already unique on disk) is folded into that alphabet.
pub fn agent_name(task_id: &str) -> String {
    let body: String = task_id
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' { c } else { '-' })
        .collect();
    let mut name = format!("task-{body}");
    name.truncate(32);
    name
}
fn load(id: &str) -> Result<Value, String> {
    let raw = std::fs::read(directory().join(format!("{}.json", key(id)?)))
        .map_err(|e| format!("task {id}: {e}"))?;
    serde_json::from_slice(&raw).map_err(|e| e.to_string())
}
fn save(record: &Value) -> Result<(), String> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let dir = directory();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| e.to_string())?;
    let id = key(str_of(record, "task_id"))?;
    let tmp = dir.join(format!(".{id}-{}.tmp", random_id()));
    let result = (|| {
        let mut f = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| e.to_string())?;
        f.write_all(&serde_json::to_vec_pretty(record).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, dir.join(format!("{id}.json"))).map_err(|e| e.to_string())?;
        std::fs::File::open(&dir)
            .and_then(|d| d.sync_all())
            .map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    result
}
struct Operation(String);
impl Operation {
    fn acquire(id: &str) -> Result<Self, String> {
        if !lock(ACTIVE.get_or_init(Default::default)).insert(id.to_owned()) {
            return Err("task_operation_in_progress: query status".into());
        }
        Ok(Self(id.into()))
    }
}
impl Drop for Operation {
    fn drop(&mut self) {
        lock(ACTIVE.get_or_init(Default::default)).remove(&self.0);
    }
}
fn git(cwd: &str, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .args(["-C", cwd])
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Resolve `spec`/`kind`/`args` into herdr `agent.start` params.
fn resolve_agent(p: &Value) -> Result<(String, Vec<String>), String> {
    let (kind, mut args) = AgentRegistry::load().resolve(str_of(p, "spec"), str_of(p, "kind"))?;
    if let Some(extra) = p.get("args") {
        for arg in extra.as_array().ok_or("args must be strings")? {
            args.push(arg.as_str().ok_or("args must be strings")?.to_owned());
        }
    }
    Ok((kind, args))
}

pub fn handle(session: &Shared, method: &str, p: &Value) -> Result<Value, String> {
    if method == "task.start" {
        return start(session, p);
    }
    if method == "task.list" {
        let mut records = vec![];
        if directory().exists() {
            for entry in std::fs::read_dir(directory()).map_err(|e| e.to_string())? {
                let path = entry.map_err(|e| e.to_string())?.path();
                if path.extension().is_some_and(|x| x == "json") {
                    let id = path.file_stem().unwrap().to_string_lossy();
                    records.push(status(load(&id)?));
                }
            }
        }
        return Ok(json!(records));
    }
    let id = key(str_of(p, "task_id"))?;
    let mut record = load(id)?;
    match method {
        "task.status" => Ok(status(record)),
        "task.read" => {
            record = status(record);
            if record["phase"] == "interrupted" || !record["pane_id"].is_string() {
                return Ok(record);
            }
            let lines = p["lines"].as_u64().unwrap_or(80).min(2000);
            // the pane outlives the agent: read by pane so a finished or
            // exited agent's last screen is still inspectable
            let read = herdr::call(
                "pane.read",
                json!({"pane_id": record["pane_id"], "source": "recent", "lines": lines}),
            )?;
            record["screen"] = read.get("text").cloned().unwrap_or(Value::Null);
            Ok(record)
        }
        "task.resume" => {
            let _op = Operation::acquire(id)?;
            record = load(id)?;
            if record["boot_id"] != boot() {
                return Err("interrupted: daemon restarted; inspect retained worktree, do not resend automatically".into());
            }
            if record["phase"] != "awaiting_ready" {
                return Err("resume only supports a retained launch whose initial prompt was never submitted".into());
            }
            launch_and_submit(&mut record, true)?;
            Ok(status(record))
        }
        "task.tell" => {
            let _op = Operation::acquire(id)?;
            record = load(id)?;
            let request = key(str_of(p, "request_id"))?;
            let text = validate_text(str_of(p, "text"))?;
            if let Some(old) = record["messages"].get(request) {
                if old["text"] != text {
                    return Err("request_id_conflict".into());
                }
                return Ok(status(record));
            }
            record = status(record);
            if record["phase"] != "settled" {
                return Err("task_not_settled: busy, blocked or uncertain tasks cannot receive follow-up text".into());
            }
            record["messages"][request] = json!({"text": text, "state": "submitting"});
            record["phase"] = json!("submitting");
            save(&record)?;
            let result = prompt(&record, &text);
            apply_receipt(&mut record, result);
            record["messages"][request]["receipt"] = record["receipt"].clone();
            record["messages"][request]["state"] = record["phase"].clone();
            save(&record)?;
            Ok(status(record))
        }
        _ => Err(format!("unknown task method {method}")),
    }
}

fn start(session: &Shared, p: &Value) -> Result<Value, String> {
    let id = key(str_of(p, "request_id"))?;
    let _op = match Operation::acquire(id) {
        Ok(op) => op,
        Err(e) => {
            if let Ok(record) = load(id) {
                if record["request"] != *p {
                    return Err("request_id_conflict".into());
                }
                return Ok(status(record));
            }
            return Err(e);
        }
    };
    let path = directory().join(format!("{id}.json"));
    if path.exists() {
        let old = load(id)?;
        if old["request"] != *p {
            return Err("request_id_conflict".into());
        }
        return Ok(status(old));
    }
    let text = validate_text(str_of(p, "text"))?;
    let (kind, args) = resolve_agent(p)?;
    let cwd = std::fs::canonicalize(str_of(p, "cwd"))
        .map_err(|e| format!("cwd: {e}"))?
        .to_string_lossy()
        .into_owned();
    let repo = git(&cwd, &["rev-parse", "--show-toplevel"])?;
    let base = if str_of(p, "base").is_empty() {
        "HEAD"
    } else {
        str_of(p, "base")
    };
    let commit = git(
        &cwd,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{base}^{{commit}}"),
        ],
    )?;
    let dirty = !git(&cwd, &["status", "--porcelain"])?.is_empty();
    if dirty && p["allow_dirty"] != true {
        return Err("dirty_source: commit/stash changes or explicitly --allow-dirty (worktree will NOT contain uncommitted changes)".into());
    }
    let branch = if str_of(p, "branch").is_empty() {
        format!("lazed/{id}")
    } else {
        str_of(p, "branch").to_owned()
    };
    git(&repo, &["check-ref-format", "--branch", &branch])?;
    if git(
        &repo,
        &["show-ref", "--verify", &format!("refs/heads/{branch}")],
    )
    .is_ok()
    {
        return Err("branch_already_exists".into());
    }
    let checkout = crate::state::worktrees_dir().join(id);
    if checkout.exists() {
        return Err("task_checkout_already_exists".into());
    }
    // herdr must be reachable before any git side effect — a worktree
    // nobody can open is a mess the caller would have to clean up by hand
    herdr::snapshot()?;
    let mut record = json!({"schema": 2, "task_id": id, "request_id": id, "request": p, "boot_id": boot(),
        "phase": "creating", "repo": repo, "base_commit": commit, "branch": branch,
        "checkout_path": checkout, "agent_params": {"kind": kind, "args": args},
        "agent_name": agent_name(id),
        "text": text, "messages": {}, "verified": false,
        "warnings": if dirty { vec!["uncommitted source changes are excluded"] } else { vec![] }});
    save(&record)?; // durable intent before any Git/process side effect
    let result = (|| {
        std::fs::create_dir_all(crate::state::worktrees_dir()).map_err(|e| e.to_string())?;
        let checkout_str = checkout.to_str().ok_or("non-UTF8 checkout")?.to_string();
        git(&repo, &["worktree", "add", "-b", &branch, &checkout_str, &commit])?;
        // open the checkout in herdr, then file it under the repo's project.
        // Git and herdr are deliberately outside the session lock.
        let (ws, tab, pane) = herdr::workspace_create(&checkout_str, Some(&branch))?;
        let (repo_root, repo_key) = Session::resolve_repo(&repo);
        let mut s = lock(session);
        let project_id = s.ensure_project(&repo_root, &repo_key, None, None)?;
        s.register_workspace(&ws, &project_id, &checkout_str, None, Some(branch.clone()), false)?;
        record["project_id"] = json!(project_id);
        record["workspace_id"] = json!(ws);
        record["tab_id"] = json!(tab);
        record["pane_id"] = json!(pane);
        record["phase"] = json!("launching");
        s.persist()?;
        drop(s);
        save(&record)?;
        launch_and_submit(&mut record, false)
    })();
    if let Err(error) = result {
        record["error"] = json!(error);
        record["phase"] = json!("failed");
        // Retain resources: never delete a checkout after an ambiguous side
        // effect or close a pane that may have started an agent.
        save(&record)?;
    }
    Ok(status(record))
}

/// Start the herdr agent in the task's pane and submit the initial prompt.
/// `resume` skips the start: the agent from an earlier `awaiting_ready`
/// launch is checked for readiness instead of being launched twice.
fn launch_and_submit(record: &mut Value, resume: bool) -> Result<(), String> {
    let name = str_of(record, "agent_name").to_string();
    if !resume {
        let params = json!({
            "name": name,
            "kind": record["agent_params"]["kind"],
            "args": record["agent_params"]["args"],
            "pane_id": record["pane_id"],
            "timeout_ms": 30_000,
        });
        match herdr::call("agent.start", params) {
            Ok(info) => record["agent"] = info,
            Err(error) => {
                // herdr keeps the name for a blocked/slow startup; anything
                // else means no agent is there to resume
                let retained = matches!(herdr::call("agent.get", json!({"target": name})), Ok(ref v) if v.get("pane_id").is_some());
                record["phase"] = json!(if retained { "awaiting_ready" } else { "failed" });
                record["error"] = json!(error);
                return save(record);
            }
        }
    } else {
        let info = herdr::call("agent.get", json!({"target": name}))
            .map_err(|e| format!("agent_gone: {e}"))?;
        if !matches!(str_of(&info, "agent_status"), "idle" | "done") {
            return Err(format!("agent_not_ready: {}", str_of(&info, "agent_status")));
        }
        record["agent"] = info;
    }
    record["phase"] = json!("submitting");
    record["error"] = Value::Null;
    save(record)?; // after this point a retry must never auto-submit
    let text = str_of(record, "text").to_string();
    let result = prompt(record, &text);
    apply_receipt(record, result);
    save(record)
}

/// herdr `agent.prompt` — bracketed paste + Enter as one ordered
/// submission; herdr refuses (`agent_blocked`/`agent_not_ready`) before
/// writing anything when the agent can't take input.
fn prompt(record: &Value, text: &str) -> Result<Value, String> {
    herdr::call(
        "agent.prompt",
        json!({"target": record["agent_name"], "text": text}),
    )
}

fn apply_receipt(record: &mut Value, result: Result<Value, String>) {
    match result {
        Ok(receipt) => {
            record["phase"] = json!("running");
            record["error"] = Value::Null;
            record["receipt"] = json!({"accepted": true, "submitted": true, "herdr": receipt});
        }
        Err(error) => {
            record["phase"] = json!("submission_uncertain");
            record["error"] = json!(error);
            record["receipt"] = Value::Null;
        }
    }
}

/// Live phase from herdr's view of the agent. A name herdr no longer knows
/// (agent exited, replaced, released) means the task is interrupted.
fn status(mut record: Value) -> Value {
    if record["boot_id"] != boot() {
        record["previous_phase"] = record["phase"].clone();
        record["phase"] = json!("interrupted");
        return record;
    }
    if !record["agent_name"].is_string() || !record["pane_id"].is_string() {
        return record;
    }
    if matches!(str_of(&record, "phase"), "creating" | "launching" | "failed") {
        return record;
    }
    match herdr::call("agent.get", json!({"target": record["agent_name"]})) {
        Ok(observed) => {
            if matches!(
                str_of(&record, "phase"),
                "running" | "settled" | "blocked" | "unknown" | "submission_uncertain"
            ) {
                record["phase"] = json!(match str_of(&observed, "agent_status") {
                    "working" => "running",
                    "idle" | "done" => "settled",
                    "blocked" => "blocked",
                    _ => "unknown",
                });
            } else if record["phase"] == "awaiting_ready"
                && matches!(str_of(&observed, "agent_status"), "idle" | "done")
                && observed["launch_pending"] != true
            {
                record["ready"] = json!(true);
            }
            record["agent"] = observed;
        }
        Err(error) => {
            record["phase"] = json!("interrupted");
            record["error"] = json!(error);
        }
    }
    record
}

/// Do not echo full prompts, launch configuration or internal persistence
/// metadata in every status response. The durable record remains intact.
pub fn public(mut value: Value) -> Value {
    if let Some(records) = value.as_array_mut() {
        for record in records {
            *record = public(record.take());
        }
    } else if let Some(record) = value.as_object_mut() {
        for field in ["request", "text", "agent_params", "boot_id"] {
            record.remove(field);
        }
        if let Some(messages) = record.get_mut("messages").and_then(Value::as_object_mut) {
            for message in messages.values_mut() {
                if let Some(message) = message.as_object_mut() {
                    message.remove("text");
                }
            }
        }
    }
    value
}

pub fn cli(args: &[String]) -> i32 {
    match cli_run(args) {
        Ok(v) => {
            println!("{}", serde_json::to_string_pretty(&v).unwrap());
            if matches!(
                str_of(&v, "phase"),
                "failed" | "submission_uncertain" | "interrupted" | "awaiting_ready"
            ) {
                1
            } else {
                0
            }
        }
        Err(e) => {
            eprintln!("lazed task: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_followup_never_inherits_a_previous_successful_receipt() {
        let mut record = json!({"phase": "settled", "receipt": {"accepted": true}});
        apply_receipt(&mut record, Err("write failed".into()));
        assert_eq!(record["phase"], "submission_uncertain");
        assert!(record["receipt"].is_null());
    }

    #[test]
    fn public_status_omits_prompt_and_internal_configuration() {
        let private = json!({"task_id": "task", "text": "secret prompt", "request": {"text": "secret prompt"},
            "boot_id": "boot", "agent_params": {}, "messages": {"next": {"text": "another prompt", "state": "running"}}});
        let public = public(private);
        assert_eq!(public["task_id"], "task");
        assert!(public.get("request").is_none());
        assert!(public.get("text").is_none());
        assert!(public["messages"]["next"].get("text").is_none());
    }

    #[test]
    fn agent_names_fit_herdr_rules() {
        assert_eq!(agent_name("Fix_Login-1"), "task-fix_login-1");
        let long = agent_name(&"a".repeat(100));
        assert_eq!(long.len(), 32);
        assert!(long.starts_with("task-"));
        assert!(agent_name("x.y").chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_'));
    }

    #[test]
    fn validate_text_rejects_control_and_empty() {
        assert!(validate_text("  \n").is_err());
        assert!(validate_text("ok\ttab\nline").is_ok());
        assert!(validate_text("bad\x1b[2J").is_err());
        assert_eq!(validate_text("a\r\nb").unwrap(), "a\nb");
    }
}

fn cli_run(args: &[String]) -> Result<Value, String> {
    let help = "usage: lazed task start [--cwd PATH] [--agent SPEC|--kind KIND] [--base REF] [--branch NAME] [--request-id ID] [--allow-dirty] [--prompt-file PATH | --stdin | -- TEXT]\n       lazed task status|read|resume TASK_ID\n       lazed task tell TASK_ID [--request-id ID] -- TEXT\n       lazed task list";
    let command = args.first().map(String::as_str).unwrap_or("--help");
    if matches!(command, "--help" | "-h") {
        return Ok(json!({"usage": help}));
    }
    if !matches!(
        command,
        "start" | "status" | "read" | "resume" | "tell" | "list"
    ) {
        return Err(help.into());
    }
    let mut params = json!({});
    let mut i = 1;
    if matches!(command, "status" | "read" | "resume" | "tell") {
        params["task_id"] = json!(args.get(i).ok_or(help)?);
        i += 1;
    }
    if command == "start" {
        params["cwd"] = json!(std::env::current_dir().map_err(|e| e.to_string())?);
    }
    let mut prompt = None;
    while i < args.len() {
        let option = args[i].as_str();
        if option == "--" {
            if prompt.is_some() {
                return Err("choose exactly one prompt source".into());
            }
            prompt = Some(args[i + 1..].join(" "));
            break;
        }
        if option == "--stdin" {
            if prompt.is_some() {
                return Err("choose exactly one prompt source".into());
            }
            let mut text = String::new();
            std::io::stdin()
                .take(65537)
                .read_to_string(&mut text)
                .map_err(|e| e.to_string())?;
            prompt = Some(text);
            i += 1;
            continue;
        }
        if option == "--allow-dirty" {
            params["allow_dirty"] = json!(true);
            i += 1;
            continue;
        }
        let field = match option {
            "--cwd" => "cwd",
            "--agent" => "spec",
            "--kind" => "kind",
            "--base" => "base",
            "--branch" => "branch",
            "--request-id" => "request_id",
            "--prompt-file" => "prompt_file",
            _ => return Err(format!("unknown option {option}; {help}")),
        };
        let value = args.get(i + 1).ok_or(help)?;
        if field == "prompt_file" {
            if prompt.is_some() {
                return Err("choose exactly one prompt source".into());
            }
            let mut text = String::new();
            std::fs::File::open(value)
                .map_err(|e| e.to_string())?
                .take(65537)
                .read_to_string(&mut text)
                .map_err(|e| e.to_string())?;
            prompt = Some(text);
        } else {
            params[field] = json!(value);
        }
        i += 2;
    }
    if matches!(command, "start" | "tell") {
        params["text"] = json!(validate_text(
            &prompt.ok_or("provide -- TEXT, --prompt-file PATH or --stdin")?
        )?);
        if params["request_id"].is_null() {
            params["request_id"] = json!(random_id());
        }
        eprintln!(
            "request_id={} (retain this ID; query status after disconnect, never blindly resend)",
            params["request_id"]
        );
    } else if prompt.is_some() {
        return Err("this command does not accept a prompt".into());
    }
    let info = crate::api_call("session.status", json!({}))?;
    if !info["capabilities"]
        .as_array()
        .is_some_and(|caps| caps.iter().any(|c| c == "task.v1"))
    {
        return Err("daemon_upgrade_required: running daemon lacks task.v1; finish active panes before restarting (do not stop it automatically)".into());
    }
    crate::api_call(&format!("task.{command}"), params)
}
