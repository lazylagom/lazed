//! Durable execution records. A task is not a claim of verified code completion.
use crate::{
    control::{self, str_of},
    server::Shared,
    session::Session,
    term::lock,
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
    BOOT.get_or_init(control::id)
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
    let tmp = dir.join(format!(".{id}-{}.tmp", control::id()));
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
                    records.push(status(session, load(&id)?));
                }
            }
        }
        return Ok(json!(records));
    }
    let id = key(str_of(p, "task_id"))?;
    let mut record = load(id)?;
    match method {
        "task.status" => Ok(status(session, record)),
        "task.read" => {
            record = status(session, record);
            if record["phase"] == "interrupted" || !record["term_id"].is_string() {
                return Ok(record);
            }
            let term = lock(session).get_terminal(str_of(&record, "term_id"))?;
            record["screen"] = json!(
                lock(&term).screen_text(p["lines"].as_u64().unwrap_or(80).min(2000) as usize)
            );
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
            launch_and_submit(session, &mut record, true)?;
            Ok(status(session, record))
        }
        "task.tell" => {
            let _op = Operation::acquire(id)?;
            record = load(id)?;
            let request = key(str_of(p, "request_id"))?;
            let text = control::validate_text(str_of(p, "text"))?;
            if let Some(old) = record["messages"].get(request) {
                if old["text"] != text {
                    return Err("request_id_conflict".into());
                }
                return Ok(status(session, record));
            }
            record = status(session, record);
            if record["phase"] != "settled" {
                return Err("task_not_settled: busy, blocked or uncertain tasks cannot receive follow-up text".into());
            }
            record["messages"][request] = json!({"text": text, "state": "submitting"});
            record["phase"] = json!("submitting");
            save(&record)?;
            let params = json!({"term_id": record["term_id"], "launch_id": record["launch_id"], "text": text});
            let result = control::handle(session, "agent.prompt", &params);
            apply_receipt(&mut record, result);
            record["messages"][request]["receipt"] = record["receipt"].clone();
            record["messages"][request]["state"] = record["phase"].clone();
            save(&record)?;
            Ok(status(session, record))
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
                return Ok(status(session, record));
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
        return Ok(status(session, old));
    }
    let text = control::validate_text(str_of(p, "text"))?;
    let (kind, args, expected) = control::resolve(p)?;
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
    let mut record = json!({"schema": 1, "task_id": id, "request_id": id, "request": p, "boot_id": boot(),
        "phase": "creating", "repo": repo, "base_commit": commit, "branch": branch,
        "checkout_path": checkout, "agent_params": {"kind": kind, "args": args, "expected_kind": expected},
        "text": text, "messages": {}, "verified": false,
        "warnings": if dirty { vec!["uncommitted source changes are excluded"] } else { vec![] }});
    save(&record)?; // durable intent before any Git/process side effect
    let result = (|| {
        std::fs::create_dir_all(crate::state::worktrees_dir()).map_err(|e| e.to_string())?;
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                &branch,
                checkout.to_str().ok_or("non-UTF8 checkout")?,
                &commit,
            ],
        )?;
        // Git is deliberately outside the global session lock.
        let (_, repo_key) = Session::resolve_repo(&repo);
        let mut s = lock(session);
        let existing = s
            .projects
            .iter()
            .find(|(_, x)| x.repo_key == repo_key)
            .map(|(id, _)| id.clone());
        let project_id = match existing {
            Some(id) => id,
            None => s.create_project(&repo, None, None)?["project"]["project_id"]
                .as_str()
                .ok_or("missing project ID")?
                .to_owned(),
        };
        let ws = s.create_workspace(
            &project_id,
            checkout.to_str().unwrap(),
            Some(branch.clone()),
            Some(branch.clone()),
            false,
        )?;
        record["project_id"] = json!(project_id);
        record["workspace_id"] = ws["workspace"]["workspace_id"].clone();
        record["term_id"] = ws["terminal"]["term_id"].clone();
        record["phase"] = json!("launching");
        s.persist()?;
        drop(s);
        save(&record)?;
        launch_and_submit(session, &mut record, false)
    })();
    if let Err(error) = result {
        record["error"] = json!(error);
        record["phase"] = json!("failed");
        // Retain resources: never delete a checkout after an ambiguous side
        // effect or kill a pane that may have started an agent.
        save(&record)?;
    }
    Ok(status(session, record))
}

fn launch_and_submit(session: &Shared, record: &mut Value, resume: bool) -> Result<(), String> {
    let mut params = record["agent_params"].clone();
    params["term_id"] = record["term_id"].clone();
    if resume {
        params["launch_id"] = record["launch_id"].clone();
    }
    let ready = control::handle(session, "agent.start", &params);
    let observed = control::handle(session, "agent.get", &params)?;
    record["launch_id"] = observed["launch_id"].clone();
    if let Err(error) = ready {
        record["phase"] = json!(if observed["launch_id"].is_string() {
            "awaiting_ready"
        } else {
            "failed"
        });
        record["error"] = json!(error);
        return save(record);
    }
    record["phase"] = json!("submitting");
    record["error"] = Value::Null;
    save(record)?; // after this point a retry must never auto-submit
    let prompt = json!({"term_id": record["term_id"], "launch_id": record["launch_id"], "text": record["text"]});
    let result = control::handle(session, "agent.prompt", &prompt);
    apply_receipt(record, result);
    save(record)
}
fn apply_receipt(record: &mut Value, result: Result<Value, String>) {
    match result {
        Ok(receipt) => {
            record["phase"] = json!(if receipt["accepted"] == true {
                "running"
            } else {
                "submission_uncertain"
            });
            record["error"] = receipt["error"].clone();
            record["receipt"] = receipt;
        }
        Err(error) => {
            record["phase"] = json!("submission_uncertain");
            record["error"] = json!(error);
            record["receipt"] = Value::Null;
        }
    }
}
fn status(session: &Shared, mut record: Value) -> Value {
    if record["boot_id"] != boot() {
        record["previous_phase"] = record["phase"].clone();
        record["phase"] = json!("interrupted");
        return record;
    }
    if !record["term_id"].is_string() {
        return record;
    }
    let params = json!({"term_id": record["term_id"]});
    match control::handle(session, "agent.get", &params) {
        Ok(observed) => {
            if record["launch_id"].is_string()
                && (record["launch_id"] != observed["launch_id"]
                    || observed["dead"] == true
                    || observed["agent"].is_null())
            {
                record["phase"] = json!("interrupted");
            } else if matches!(
                str_of(&record, "phase"),
                "running" | "settled" | "blocked" | "submission_uncertain"
            ) {
                let active = observed["activity_seq"].as_u64().unwrap_or(0)
                    > record["receipt"]["after_seq"].as_u64().unwrap_or(u64::MAX);
                if active {
                    record["phase"] = json!(match str_of(&observed, "agent_status") {
                        "working" => "running",
                        "idle" | "done" => "settled",
                        "blocked" => "blocked",
                        _ => "unknown",
                    });
                }
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
        let mut record = json!({"phase": "settled", "receipt": {"accepted": true, "after_seq": 1}});
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
}
fn cli_run(args: &[String]) -> Result<Value, String> {
    let help = "usage: lazed task start [--cwd PATH] [--agent SPEC] [--base REF] [--branch NAME] [--request-id ID] [--allow-dirty] [--prompt-file PATH | --stdin | -- TEXT]\n       lazed task status|read|resume TASK_ID\n       lazed task tell TASK_ID [--request-id ID] -- TEXT\n       lazed task list";
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
        params["text"] = json!(control::validate_text(
            &prompt.ok_or("provide -- TEXT, --prompt-file PATH or --stdin")?
        )?);
        if params["request_id"].is_null() {
            params["request_id"] = json!(control::id());
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
