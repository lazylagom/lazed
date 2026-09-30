//! `lazed init` — install the `crew` skill into a project. The skill ships
//! inside this binary, so `lazed init` and the app's import write the same
//! files wherever lazed runs from. `.agents/skills/crew/` is the one real
//! copy: codex, gemini, pi and devin read it there. Claude Code reads only
//! `.claude/skills/`, so that path gets a relative symlink to the same
//! directory. The files are committable, and existing ones are never
//! overwritten, because `crew.md` is the project's own stage definition.

use serde_json::{json, Value};
use std::path::Path;

const SKILL_MD: &str = include_str!("../../skills/crew/SKILL.md");
const CREW_MD: &str = include_str!("../../skills/crew/crew.md");

const SKILL_DIR: &str = ".agents/skills/crew";
const CLAUDE_LINK: &str = ".claude/skills/crew";
const CLAUDE_TARGET: &str = "../../.agents/skills/crew";

/// Install into `root`. With `force`, SKILL.md is refreshed to this build's
/// version. `crew.md` is never overwritten, because it holds the user's edits.
pub fn init(root: &Path, force: bool) -> Result<Value, String> {
    if !root.is_dir() {
        return Err(format!("not a directory: {}", root.display()));
    }
    let mut installed = vec![];
    let mut skipped = vec![];
    let dir = root.join(SKILL_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for (name, body, refresh) in [("SKILL.md", SKILL_MD, force), ("crew.md", CREW_MD, false)] {
        let path = dir.join(name);
        let rel = format!("{SKILL_DIR}/{name}");
        if path.exists() && !refresh {
            skipped.push(rel);
            continue;
        }
        std::fs::write(&path, body).map_err(|e| format!("{}: {e}", path.display()))?;
        installed.push(rel);
    }
    let link = root.join(CLAUDE_LINK);
    if std::fs::symlink_metadata(&link).is_ok() {
        // a real dir or someone else's link — leave it alone
        skipped.push(CLAUDE_LINK.to_string());
    } else {
        let parent = link.parent().unwrap();
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        std::os::unix::fs::symlink(CLAUDE_TARGET, &link)
            .map_err(|e| format!("{}: {e}", link.display()))?;
        installed.push(CLAUDE_LINK.to_string());
    }
    Ok(json!({"root": root.to_string_lossy(), "installed": installed, "skipped": skipped}))
}

/// Where `lazed init` installs: the checkout `cwd` is in, not the repo's
/// main checkout. Uncommitted files don't show up in other worktrees, and
/// an agent running in a worktree reads only that worktree's files.
pub fn checkout_root(cwd: &str) -> String {
    crate::repo::resolve(cwd).map_or_else(|| cwd.to_string(), |r| r.checkout)
}

pub fn cli(args: &[String]) -> i32 {
    let mut force = false;
    let mut path: Option<&str> = None;
    for a in args {
        match a.as_str() {
            "--force" => force = true,
            "--help" | "-h" => {
                eprintln!("usage: lazed init [--force] [path]   install the crew skill into a project");
                return 0;
            }
            p if !p.starts_with('-') && path.is_none() => path = Some(p),
            other => {
                eprintln!("lazed init: unexpected argument {other}");
                return 2;
            }
        }
    }
    let cwd = match path {
        Some(p) => p.to_string(),
        None => std::env::current_dir()
            .map(|d| d.to_string_lossy().into_owned())
            .unwrap_or_else(|_| ".".into()),
    };
    let cwd = match std::fs::canonicalize(&cwd) {
        Ok(p) => p.to_string_lossy().into_owned(),
        Err(e) => {
            eprintln!("lazed init: {cwd}: {e}");
            return 1;
        }
    };
    match init(Path::new(&checkout_root(&cwd)), force) {
        Ok(v) => {
            for p in v["installed"].as_array().into_iter().flatten() {
                println!("installed  {}", p.as_str().unwrap_or_default());
            }
            for p in v["skipped"].as_array().into_iter().flatten() {
                println!("  exists   {}", p.as_str().unwrap_or_default());
            }
            println!("crew skill ready in {}", v["root"].as_str().unwrap_or_default());
            0
        }
        Err(e) => {
            eprintln!("lazed init: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("lazed-crew-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn installs_one_copy_and_a_claude_link() {
        let root = tmp("fresh");
        let v = init(&root, false).unwrap();
        assert_eq!(v["installed"].as_array().unwrap().len(), 3);
        let skill = root.join(".agents/skills/crew/SKILL.md");
        assert!(std::fs::read_to_string(&skill).unwrap().contains("name: crew"));
        let link = root.join(".claude/skills/crew");
        assert_eq!(std::fs::read_link(&link).unwrap(), Path::new(CLAUDE_TARGET));
        assert!(link.join("crew.md").is_file(), "link must resolve to the real dir");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn second_init_keeps_edits_and_force_spares_crew_md() {
        let root = tmp("again");
        init(&root, false).unwrap();
        let crew = root.join(".agents/skills/crew/crew.md");
        let skill = root.join(".agents/skills/crew/SKILL.md");
        std::fs::write(&crew, "mine").unwrap();
        std::fs::write(&skill, "old").unwrap();

        let v = init(&root, false).unwrap();
        assert!(v["installed"].as_array().unwrap().is_empty());
        assert_eq!(std::fs::read_to_string(&crew).unwrap(), "mine");

        init(&root, true).unwrap();
        assert_eq!(std::fs::read_to_string(&crew).unwrap(), "mine");
        assert_eq!(std::fs::read_to_string(&skill).unwrap(), SKILL_MD);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn existing_claude_dir_is_left_alone() {
        let root = tmp("claudedir");
        let real = root.join(".claude/skills/crew");
        std::fs::create_dir_all(&real).unwrap();
        let v = init(&root, false).unwrap();
        assert!(v["skipped"].as_array().unwrap().iter().any(|p| p == CLAUDE_LINK));
        assert!(!std::fs::symlink_metadata(&real).unwrap().file_type().is_symlink());
        let _ = std::fs::remove_dir_all(&root);
    }
}
