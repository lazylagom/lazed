use std::process::Command;

use serde_json::{json, Value};

fn git(dir: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run git {:?}: {e}", args))?;
    if !out.status.success() {
        return Err(format!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn git_ok(dir: &str, args: &[&str]) -> Option<String> {
    git(dir, args).ok().map(|s| s.trim().to_string())
}

/// Detect the repo's likely base branch: origin/HEAD → main → master.
fn detect_base(dir: &str, current: &str) -> Option<String> {
    if let Some(head) = git_ok(
        dir,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    ) {
        let b = head.strip_prefix("origin/").unwrap_or(&head).to_string();
        if b != current {
            return Some(b);
        }
    }
    for cand in ["main", "master"] {
        if cand != current && git_ok(dir, &["rev-parse", "--verify", cand]).is_some() {
            return Some(cand.to_string());
        }
    }
    None
}

/// Everything the worktree changed vs its fork point: `git diff <merge-base>`
/// covers committed-on-branch + staged + unstaged. Untracked files listed
/// separately (their contents are new-file only).
pub fn worktree_diff(checkout: &str, base: Option<&str>) -> Result<Value, String> {
    let branch = git(checkout, &["rev-parse", "--abbrev-ref", "HEAD"])?
        .trim()
        .to_string();
    let base = base
        .map(str::to_string)
        .or_else(|| detect_base(checkout, &branch))
        .filter(|b| b != &branch);

    let diff = if let Some(b) = &base {
        match git_ok(checkout, &["merge-base", b, "HEAD"]) {
            Some(mb) => git(checkout, &["diff", &mb])?,
            None => git(checkout, &["diff", b])?,
        }
    } else {
        // no base known: show working-tree changes vs HEAD
        git(checkout, &["diff", "HEAD"])?
    };

    let stat = if let Some(b) = &base {
        git_ok(checkout, &["merge-base", b, "HEAD"])
            .and_then(|mb| git_ok(checkout, &["diff", "--stat", &mb]))
    } else {
        git_ok(checkout, &["diff", "--stat", "HEAD"])
    };

    let status = git(checkout, &["status", "--porcelain"])?;
    let untracked: Vec<String> = status
        .lines()
        .filter_map(|l| l.strip_prefix("?? ").map(str::to_string))
        .collect();

    Ok(json!({
        "branch": branch,
        "base": base,
        "diff": diff,
        "stat": stat.unwrap_or_default(),
        "untracked": untracked,
    }))
}

/// Merge a worktree branch into the checkout at `repo` (no-ff so the result
/// is attributable). Conflict surfaces as an error string for the UI.
pub fn worktree_merge(repo: &str, branch: &str) -> Result<Value, String> {
    match git(repo, &["merge", "--no-ff", "-m", &format!("lazed: merge {branch}"), branch]) {
        Ok(out) => Ok(json!({"ok": true, "output": out})),
        Err(e) => Ok(json!({"ok": false, "output": e})),
    }
}

/// Delete the branch a removed worktree was on — herdr's `worktree.remove`
/// leaves it behind by design. `-d` refuses to drop unmerged work; `force`
/// switches to `-D`. A missing branch is success: "no such branch" is the
/// goal state either way.
pub fn branch_delete(repo: &str, branch: &str, force: bool) -> Result<Value, String> {
    let flag = if force { "-D" } else { "-d" };
    match git(repo, &["branch", flag, branch]) {
        Ok(out) => Ok(json!({"ok": true, "output": out})),
        Err(e) if e.contains("not found") || e.contains("no such branch") => {
            Ok(json!({"ok": true, "output": e}))
        }
        Err(e) => Ok(json!({"ok": false, "output": e})),
    }
}

/// Resolve a directory's repo identity for project grouping/import dedupe.
/// `repo_key` = the shared git dir (for a linked worktree it resolves to the
/// main repo's .git, so importing a worktree path maps to the same project).
/// `repo_root` = the main checkout root. Ok(None) for non-repo paths.
pub fn resolve_repo(dir: &str) -> Result<Option<Value>, String> {
    let Some(repo) = crate::repo::resolve(dir) else { return Ok(None) };
    let root = repo.main.unwrap_or(repo.checkout);
    let name = std::path::Path::new(&root).file_name().map(|s| s.to_string_lossy().into_owned());
    Ok(Some(json!({"repo_key": repo.key, "repo_root": root, "name": name})))
}
