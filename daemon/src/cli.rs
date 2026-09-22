//! Human-sized worktree commands for agents. Pane and agent control are
//! herdr's (`herdr pane …`, `herdr agent …`); lazed only adds the checkout
//! lifecycle herdr doesn't manage the same way (hardened removal, project
//! filing). JSON APIs remain available underneath.
use serde_json::{json, Value};

const WORKTREE_HELP: &str = "lazed worktree create --branch NAME [--repo PATH] [--base REF] [--path PATH] [--allow-dirty]\n  worktree list [--repo PATH]\n  worktree remove WORKSPACE_ID [--force] [--kill-agents] [--keep-branch]\nCreate adds a git worktree and opens it as a herdr workspace (shell pane, no agent) under the repo's lazed project; the result carries the herdr workspace/tab/pane ids — start an agent with `herdr agent start NAME --kind KIND --pane <pane_id>`. Default repo is the caller cwd. Uncommitted files are never copied. Remove refuses while a pane still hosts an agent unless --kill-agents is given.";

pub fn run(args: &[String]) -> i32 {
    let cwd = match std::env::current_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let (method, params) = match parse(args, &cwd.to_string_lossy()) {
        Ok(request) => request,
        Err(error) => {
            eprintln!("{error}\n{WORKTREE_HELP}");
            return 2;
        }
    };
    if method == "help" {
        println!("{WORKTREE_HELP}");
        return 0;
    }
    match crate::api_call(&method, params) {
        Ok(value) => {
            println!("{}", serde_json::to_string_pretty(&value).unwrap());
            if value["error"].is_string() { 1 } else { 0 }
        }
        Err(error) => {
            eprintln!("lazed worktree: {error}");
            1
        }
    }
}

fn parse(args: &[String], cwd: &str) -> Result<(String, Value), String> {
    let sub = args.first().map(String::as_str).unwrap_or("--help");
    if matches!(sub, "--help" | "-h") || args.get(1).is_some_and(|a| a == "--help") {
        return Ok(("help".into(), Value::Null));
    }
    let mut p = json!({});
    let method: String = match sub {
        "create" => {
            p["strict"] = json!(true);
            "workspace.create".into()
        }
        "list" => "worktree.list".into(),
        "remove" => "workspace.remove".into(),
        _ => return Err(format!("unknown command worktree {sub}")),
    };
    let mut i = 1;
    if sub == "remove" {
        let value = args
            .get(i)
            .filter(|s| !s.starts_with('-'))
            .ok_or("missing workspace ID")?;
        p["workspace_id"] = json!(value);
        i += 1;
    }
    while i < args.len() {
        let arg = args[i].as_str();
        let field = match arg {
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
            "--kill-agents" if method == "workspace.remove" => {
                p["kill_agents"] = json!(true);
                i += 1;
                continue;
            }
            "--keep-branch" if method == "workspace.remove" => {
                p["keep_branch"] = json!(true);
                i += 1;
                continue;
            }
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
        p[field] = json!(value);
        i += 2;
    }
    if matches!(method.as_str(), "workspace.create" | "worktree.list") && p["repo"].is_null() {
        p["repo"] = json!(cwd);
    }
    if method == "workspace.create" && !p["branch"].is_string() {
        return Err("--branch is required".into());
    }
    // Resolve paths in the caller, never in the daemon's startup directory.
    for field in ["repo", "path"] {
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
    fn parsed(args: &[&str]) -> Result<(String, Value), String> {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>(), "/caller/subdir")
    }
    #[test]
    fn discovery_never_creates_layout() {
        assert_eq!(parsed(&[]).unwrap().0, "help");
        assert_eq!(parsed(&["create", "--help"]).unwrap().0, "help");
        assert!(parsed(&["create"]).is_err());
    }
    #[test]
    fn paths_resolve_against_caller() {
        let (m, p) = parsed(&["create", "--repo", "..", "--branch", "fix", "--path", "../fix"]).unwrap();
        assert_eq!(m, "workspace.create");
        assert_eq!(p["repo"], "/caller/subdir/..");
        assert_eq!(p["path"], "/caller/subdir/../fix");
        assert_eq!(p["strict"], true);
        let (_, p) = parsed(&["list"]).unwrap();
        assert_eq!(p["repo"], "/caller/subdir");
    }
    #[test]
    fn remove_flags() {
        let (m, p) = parsed(&["remove", "w3", "--force", "--kill-agents"]).unwrap();
        assert_eq!(m, "workspace.remove");
        assert_eq!(p["workspace_id"], "w3");
        assert_eq!(p["force"], true);
        assert_eq!(p["kill_agents"], true);
        assert!(p["keep_branch"].is_null());
        assert!(parsed(&["remove", "--force"]).is_err());
    }
}
