//! Human-sized agent and layout commands. JSON APIs remain available underneath.
use serde_json::{json, Value};
use std::io::Read;

const AGENT_HELP: &str = "lazed agent list | specs\n  agent start NAME --kind KIND|--spec SPEC --pane ID [--timeout MS] [-- NATIVE_ARGS...]\n  agent get NAME|PANE\n  agent read NAME|PANE [--lines N] [--source visible|recent|recent-unwrapped|detection]\n  agent prompt NAME|PANE TEXT [--wait] [--timeout MS]\n  agent prompt NAME|PANE --prompt-file PATH|--stdin [--wait] [--timeout MS]\n  agent wait NAME|PANE [--until idle|done|blocked|working] [--timeout MS]\n  agent send-keys NAME|PANE KEY...\nSupported managed kinds: claude, codex, devin, pi. Start uses an existing pane; it never creates a pane/worktree. Wait defaults to 30 seconds; max 600000 ms.";
const PANE_HELP: &str = "lazed pane list\n  pane current --current\n  pane split --current|--pane ID [--cwd PATH] [--no-focus]\n  pane read ID [--lines N] [--source recent-unwrapped]\nSplit appends a sibling in the same tab, preserving focus. Current lazed layout uses rows of panes, not directional split trees.";
const WORKTREE_HELP: &str = "lazed worktree create --branch NAME [--repo PATH] [--base REF] [--path PATH] [--allow-dirty]\n  worktree list [--repo PATH]\n  worktree remove WORKSPACE_ID [--force]\nCreate opens a workspace and shell pane, not an agent. Default repo is the caller cwd. Uncommitted files are never copied.";

fn help(group: &str) -> &'static str {
    match group {
        "agent" => AGENT_HELP,
        "pane" => PANE_HELP,
        _ => WORKTREE_HELP,
    }
}

pub fn run(group: &str, args: &[String]) -> i32 {
    let cwd = match std::env::current_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let request = parse(
        group,
        args,
        std::env::var("LAZED_TERM").ok().as_deref(),
        &cwd.to_string_lossy(),
    );
    let (method, mut params) = match request {
        Ok(request) => request,
        Err(error) => {
            eprintln!("{error}\n{}", help(group));
            return 2;
        }
    };
    if method == "help" {
        println!("{}", help(group));
        return 0;
    }
    if method == "agent.prompt" {
        let input = if let Some(path) = params.get("prompt_file").and_then(Value::as_str) {
            std::fs::File::open(path).map(|file| Box::new(file) as Box<dyn Read>)
        } else {
            Ok(Box::new(std::io::stdin()) as Box<dyn Read>)
        };
        if params["stdin"] == true || params["prompt_file"].is_string() {
            let result = input.and_then(|input| {
                let mut text = String::new();
                input.take(65537).read_to_string(&mut text)?;
                Ok(text)
            });
            match result {
                Ok(text) => params["text"] = json!(text),
                Err(e) => {
                    eprintln!("prompt input: {e}");
                    return 2;
                }
            }
            params.as_object_mut().unwrap().remove("stdin");
            params.as_object_mut().unwrap().remove("prompt_file");
        }
    }
    let status = crate::api_call("session.status", json!({}));
    if !status.as_ref().is_ok_and(|s| {
        s["capabilities"]
            .as_array()
            .is_some_and(|c| c.iter().any(|c| c == "agent.names.v1"))
    }) {
        eprintln!("{}", status.err().unwrap_or_else(|| "daemon_upgrade_required: finish active panes before restarting; do not stop it automatically".into()));
        return 1;
    }
    match crate::api_call(&method, params) {
        Ok(value) => {
            println!("{}", serde_json::to_string_pretty(&value).unwrap());
            if value["error"].is_string() {
                1
            } else {
                0
            }
        }
        Err(error) => {
            eprintln!("lazed {group}: {error}");
            1
        }
    }
}

fn parse(
    group: &str,
    args: &[String],
    caller: Option<&str>,
    cwd: &str,
) -> Result<(String, Value), String> {
    let sub = args.first().map(String::as_str).unwrap_or("--help");
    if matches!(sub, "--help" | "-h") || args.get(1).is_some_and(|a| a == "--help") {
        return Ok(("help".into(), Value::Null));
    }
    let mut p = json!({});
    let method = match (group, sub) {
        ("agent", "list" | "specs" | "start" | "get" | "read" | "prompt" | "wait" | "release") => {
            format!("agent.{sub}")
        }
        ("agent", "send-keys") => "agent.send_keys".into(),
        ("pane", "split") => "pane.split".into(),
        ("pane", "current") => "pane.get".into(),
        ("pane", "list") => "terminal.list".into(),
        ("pane", "read") => "agent.read".into(),
        ("worktree", "create") => {
            p["strict"] = json!(true);
            "workspace.create".into()
        }
        ("worktree", "list") => "worktree.list".into(),
        ("worktree", "remove") => "workspace.remove".into(),
        _ => return Err(format!("unknown command {group} {sub}")),
    };
    let mut i = 1;
    if group == "agent" && !matches!(sub, "list" | "specs")
        || group == "pane" && sub == "read"
        || group == "worktree" && sub == "remove"
    {
        let value = args
            .get(i)
            .filter(|s| !s.starts_with('-'))
            .ok_or("missing agent name or pane/workspace ID")?;
        let key = if sub == "start" {
            "name"
        } else if group == "worktree" {
            "workspace_id"
        } else {
            "target"
        };
        p[key] = json!(value);
        i += 1;
    }
    if method == "agent.send_keys" {
        if i == args.len() {
            return Err("at least one logical key is required".into());
        }
        p["keys"] = json!(&args[i..]);
        return Ok((method, p));
    }
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "--" {
            if method == "agent.start" {
                p["args"] = json!(&args[i + 1..]);
            } else if method == "agent.prompt" && p["text"].is_null() {
                p["text"] = json!(args[i + 1..].join(" "));
            } else {
                return Err("unexpected -- or duplicate prompt".into());
            }
            break;
        }
        if !arg.starts_with('-') && method == "agent.prompt" && p["text"].is_null() {
            p["text"] = json!(arg);
            i += 1;
            continue;
        }
        match arg {
            "--current" if group == "pane" && matches!(sub, "split" | "current") => {
                if p["term_id"].is_string() {
                    return Err("choose --current or --pane, not both".into());
                }
                p["term_id"] = json!(caller.filter(|s| !s.is_empty()).ok_or(
                    "--current requires LAZED_TERM; provide --pane from pane list instead"
                )?);
                i += 1;
                continue;
            }
            "--no-focus" if method == "pane.split" => {
                i += 1;
                continue;
            }
            "--wait" if method == "agent.prompt" => {
                p["wait"] = json!(true);
                i += 1;
                continue;
            }
            "--stdin" if method == "agent.prompt" => {
                p["stdin"] = json!(true);
                i += 1;
                continue;
            }
            "--allow-dirty" if method == "workspace.create" => {
                p["allow_dirty"] = json!(true);
                i += 1;
                continue;
            }
            "--force" if method == "workspace.remove" => {
                p["force"] = json!(true);
                i += 1;
                continue;
            }
            _ => {}
        }
        let field = match arg {
            "--kind" | "--spec" if method == "agent.start" => &arg[2..],
            "--pane" if matches!(method.as_str(), "agent.start" | "pane.split") => "term_id",
            "--timeout"
                if matches!(
                    method.as_str(),
                    "agent.start" | "agent.wait" | "agent.prompt"
                ) =>
            {
                "timeout_ms"
            }
            "--lines" | "--source" if method == "agent.read" => &arg[2..],
            "--until" if method == "agent.wait" => "until",
            "--prompt-file" if method == "agent.prompt" => "prompt_file",
            "--cwd" if method == "pane.split" => "cwd",
            "--repo" if matches!(method.as_str(), "workspace.create" | "worktree.list") => "repo",
            "--branch" | "--base" | "--path" if method == "workspace.create" => &arg[2..],
            _ => return Err(format!("unsupported option {arg}")),
        };
        let value = args
            .get(i + 1)
            .ok_or_else(|| format!("{arg} needs a value"))?;
        if !p[field].is_null() {
            return Err(format!("duplicate option {arg}"));
        }
        p[field] = if matches!(field, "timeout_ms" | "lines") {
            let n: u64 = value
                .parse()
                .map_err(|_| format!("invalid number for {arg}"))?;
            if n == 0 || n > if field == "lines" { 2000 } else { 600_000 } {
                return Err(format!("out of range {arg}"));
            }
            json!(n)
        } else if field == "until" {
            if !matches!(
                value.as_str(),
                "idle" | "done" | "blocked" | "working" | "unknown"
            ) {
                return Err("invalid --until state".into());
            }
            json!([value])
        } else {
            json!(value)
        };
        i += 2;
    }
    if matches!(method.as_str(), "agent.start" | "pane.split" | "pane.get")
        && !p["term_id"].is_string()
    {
        return Err(
            "explicit --pane or --current required; never targeting GUI focus implicitly".into(),
        );
    }
    if method == "agent.start" && p["kind"].is_string() && p["spec"].is_string() {
        return Err("choose --kind or --spec".into());
    }
    if method == "pane.split" && p["cwd"].is_null() {
        p["cwd"] = json!(cwd);
    }
    if matches!(method.as_str(), "workspace.create" | "worktree.list") && p["repo"].is_null() {
        p["repo"] = json!(cwd);
    }
    if method == "workspace.create" && !p["branch"].is_string() {
        return Err("--branch is required".into());
    }
    if method == "agent.prompt" {
        let sources = p["text"].is_string() as u8
            + p["prompt_file"].is_string() as u8
            + (p["stdin"] == true) as u8;
        if sources != 1 {
            return Err("provide exactly one prompt: TEXT, --prompt-file PATH, or --stdin".into());
        }
    }
    // Resolve paths in the caller, never in the daemon's startup directory.
    for field in ["repo", "cwd", "path"] {
        if let Some(value) = p[field].as_str() {
            if !std::path::Path::new(value).is_absolute() {
                p[field] = json!(std::path::Path::new(cwd).join(value));
            }
        }
    }
    Ok((method, p))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parsed(group: &str, args: &[&str], caller: Option<&str>) -> Result<(String, Value), String> {
        parse(
            group,
            &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            caller,
            "/caller/subdir",
        )
    }
    #[test]
    fn discovery_never_creates_layout() {
        for group in ["agent", "pane", "worktree"] {
            assert_eq!(parsed(group, &[], None).unwrap().0, "help");
        }
        assert!(parsed("agent", &["start", "reviewer", "--kind", "codex"], None).is_err());
    }
    #[test]
    fn current_is_caller_not_focused_pane() {
        assert!(parsed("pane", &["split", "--current"], None).is_err());
        let (_, p) = parsed("pane", &["split", "--current", "--no-focus"], Some("t2")).unwrap();
        assert_eq!(p["term_id"], "t2");
        assert_eq!(p["cwd"], "/caller/subdir");
        let (_, p) = parsed(
            "worktree",
            &[
                "create", "--repo", "..", "--branch", "fix", "--path", "../fix",
            ],
            None,
        )
        .unwrap();
        assert_eq!(p["repo"], "/caller/subdir/..");
        assert_eq!(p["path"], "/caller/subdir/../fix");
    }
    #[test]
    fn prompt_and_native_flags_are_kept_verbatim() {
        let (_, p) = parsed(
            "agent",
            &[
                "prompt",
                "reviewer",
                "한글\n'quote'",
                "--wait",
                "--timeout",
                "120000",
            ],
            None,
        )
        .unwrap();
        assert_eq!(p["text"], "한글\n'quote'");
        assert_eq!(p["wait"], true);
        let (_, p) = parsed(
            "agent",
            &[
                "start", "reviewer", "--pane", "t2", "--", "--model", "example",
            ],
            None,
        )
        .unwrap();
        assert_eq!(p["args"], json!(["--model", "example"]));
        assert!(parsed("agent", &["prompt", "reviewer", "hello", "--stdin"], None).is_err());
    }
}
