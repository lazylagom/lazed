//! `lazed install | uninstall | doctor` — manage the files lazed puts on a
//! machine outside its own state dir: the `~/.local/bin/lazed` symlink and
//! the agent-skill links. `install` records what it creates in
//! `<state>/install.json`; `uninstall` removes only what that manifest lists
//! — or, without one, asks before touching the known paths. `--purge` also
//! wipes state, config, worktrees, and app data.

use serde_json::{json, Value};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use crate::state;

const SKILL_LINKS: [&str; 2] = [".agents/skills/lazed", ".claude/skills/lazed"];
const HERDR_LEFTOVERS: [&str; 4] = [
    ".local/bin/herdr",
    ".herdr",
    ".local/state/herdr",
    ".config/herdr",
];
const MANAGED_AGENTS: [&str; 4] = ["claude", "codex", "devin", "pi"];

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()))
}

fn cli_path() -> PathBuf {
    home().join(".local/bin/lazed")
}

fn manifest_path() -> PathBuf {
    state::state_dir().join("install.json")
}

fn self_exe() -> Result<PathBuf, String> {
    std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map_err(|e| format!("cannot resolve own executable: {e}"))
}

/// Where the skill source lives relative to this binary: inside the app
/// bundle, a source checkout, or an explicit override — in that order.
fn skills_source(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    for c in [
        dir.join("skills/lazed"), // lazed.app/Contents/Resources/lazed
        dir.join("../skills/lazed"), // lazed.app/Contents/Resources/bin/lazed
        dir.join("../../../skills/lazed"), // daemon/target/<profile>/lazed → repo
    ] {
        if c.join("SKILL.md").is_file() {
            return c.canonicalize().ok();
        }
    }
    std::env::var("LAZED_SKILLS_DIR")
        .ok()
        .map(PathBuf::from)
        .filter(|p| p.join("SKILL.md").is_file())
        .and_then(|p| p.canonicalize().ok())
}

/// Resolve a symlink's target to an absolute canonical path (the stored
/// target may be relative to the link's directory).
fn link_target(link: &Path) -> Option<PathBuf> {
    let raw = std::fs::read_link(link).ok()?;
    let abs = if raw.is_absolute() {
        raw
    } else {
        link.parent()?.join(raw)
    };
    Some(abs.canonicalize().unwrap_or(abs))
}

fn confirm(prompt: &str, yes: bool) -> bool {
    if yes {
        return true;
    }
    if !std::io::stdin().is_terminal() {
        return false;
    }
    eprint!("{prompt} [y/N] ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).is_ok()
        && matches!(line.trim(), "y" | "Y" | "yes" | "YES")
}

/// Create `link -> target`, leaving anything we don't own alone.
fn ensure_link(link: &Path, target: &Path, yes: bool, dry: bool) -> Result<&'static str, String> {
    match std::fs::symlink_metadata(link) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if dry {
                return Ok("would link");
            }
            if let Some(p) = link.parent() {
                std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
            }
            std::os::unix::fs::symlink(target, link)
                .map_err(|e| format!("{}: {e}", link.display()))?;
            Ok("linked")
        }
        Err(e) => Err(format!("{}: {e}", link.display())),
        Ok(md) if md.file_type().is_symlink() => {
            if link_target(link).as_deref() == Some(target) {
                return Ok("already");
            }
            let cur = std::fs::read_link(link)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "?".into());
            if !confirm(
                &format!(
                    "{} -> {} (not {}); replace?",
                    link.display(),
                    cur,
                    target.display()
                ),
                yes,
            ) {
                return Err(format!(
                    "{} points at {cur}; rerun with --yes to replace",
                    link.display()
                ));
            }
            if dry {
                return Ok("would replace");
            }
            std::fs::remove_file(link).map_err(|e| format!("{}: {e}", link.display()))?;
            std::os::unix::fs::symlink(target, link)
                .map_err(|e| format!("{}: {e}", link.display()))?;
            Ok("replaced")
        }
        Ok(_) => Err(format!(
            "{} exists and is not a symlink — leaving it alone",
            link.display()
        )),
    }
}

fn read_manifest() -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(manifest_path()).ok()?).ok()
}

fn record(entries: &mut Vec<Value>, link: &Path, target: &Path) {
    let p = link.to_string_lossy().to_string();
    entries.retain(|e| e["path"].as_str() != Some(p.as_str()));
    entries.push(json!({"path": p, "target": target.to_string_lossy()}));
}

fn path_on_path_env(dir: &Path) -> bool {
    std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .any(|p| {
            let p = Path::new(p);
            p == dir || p.canonicalize().ok().as_deref() == dir.canonicalize().ok().as_deref()
        })
}

pub fn install(args: &[String]) -> i32 {
    let mut skills_only = false;
    let mut bin_only = false;
    let mut dry = false;
    let mut yes = false;
    for a in args {
        match a.as_str() {
            "--skills-only" => skills_only = true,
            "--bin-only" => bin_only = true,
            "--dry-run" => dry = true,
            "--yes" | "-y" => yes = true,
            "--help" | "-h" => {
                eprintln!("usage: lazed install [--skills-only] [--bin-only] [--dry-run] [--yes]");
                return 0;
            }
            _ => {
                eprintln!("lazed install: unknown option {a}");
                return 2;
            }
        }
    }
    if skills_only && bin_only {
        eprintln!("lazed install: choose --skills-only or --bin-only, not both");
        return 2;
    }
    let exe = match self_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("lazed install: {e}");
            return 1;
        }
    };
    // manifest = what we made; keep prior entries whose paths still exist so
    // a re-install doesn't forget links it placed earlier
    let mut entries: Vec<Value> = Vec::new();
    if let Some(old) = read_manifest() {
        if let Some(list) = old["created"].as_array() {
            for e in list {
                let keep = e["path"]
                    .as_str()
                    .is_some_and(|p| std::fs::symlink_metadata(p).is_ok());
                if keep {
                    entries.push(e.clone());
                }
            }
        }
    }
    let mut failed = false;
    let mut try_link = |link: PathBuf, target: &Path, entries: &mut Vec<Value>| {
        match ensure_link(&link, target, yes, dry) {
            Ok(s) => {
                println!("{s:>9}  {} -> {}", link.display(), target.display());
                if !dry && !s.starts_with("would") {
                    record(entries, &link, target);
                }
            }
            Err(e) => {
                eprintln!("lazed install: {e}");
                failed = true;
            }
        }
    };
    if !skills_only {
        try_link(cli_path(), &exe, &mut entries);
    }
    if !bin_only {
        match skills_source(&exe) {
            Some(src) => {
                for rel in SKILL_LINKS {
                    try_link(home().join(rel), &src, &mut entries);
                }
            }
            None => {
                eprintln!(
                    "lazed install: skill source not found near {} — set LAZED_SKILLS_DIR",
                    exe.display()
                );
                failed = true;
            }
        }
    }
    if failed {
        return 1;
    }
    if !dry {
        if state::ensure_dir().is_ok() {
            let m = json!({
                "version": 1,
                "exe": exe.to_string_lossy(),
                "lazed_version": env!("CARGO_PKG_VERSION"),
                "created": entries,
            });
            if let Err(e) = std::fs::write(
                manifest_path(),
                serde_json::to_string_pretty(&m).unwrap_or_default(),
            ) {
                eprintln!("lazed install: cannot write {}: {e}", manifest_path().display());
                return 1;
            }
        }
        if !skills_only && !path_on_path_env(&cli_path().parent().unwrap().to_path_buf()) {
            println!("note: ~/.local/bin is not on PATH — add it to use `lazed` from a shell");
        }
        println!("manifest: {}", manifest_path().display());
    }
    0
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// ~/Library payload dirs macOS creates for a running Tauri app.
fn app_data_paths() -> Vec<PathBuf> {
    let lib = home().join("Library");
    [
        "Application Support/com.lazed.app",
        "Caches/com.lazed.app",
        "WebKit/com.lazed.app",
        "HTTPStorages/com.lazed.app",
        "Saved Application State/com.lazed.app.savedState",
        "Preferences/com.lazed.app.plist",
    ]
    .iter()
    .map(|p| lib.join(p))
    .collect()
}

/// rm -rf `dir` and report; a file (not dir) still gets removed.
fn remove_path(p: &Path) {
    let md = match std::fs::symlink_metadata(p) {
        Ok(md) => md,
        Err(_) => return,
    };
    let r = if md.is_dir() && !md.file_type().is_symlink() {
        std::fs::remove_dir_all(p)
    } else {
        std::fs::remove_file(p)
    };
    match r {
        Ok(()) => println!("  removed {}", p.display()),
        Err(e) => eprintln!("  could not remove {}: {e}", p.display()),
    }
}

/// `uninstall --purge`: wipe state, config, worktrees, app data. Worktrees
/// with uncommitted changes (or that git can't verify) block the purge
/// unless --yes.
fn purge_check(yes: bool) -> Result<Vec<PathBuf>, String> {
    let wt_root = state::worktrees_dir();
    let mut blocked: Vec<(PathBuf, &str)> = Vec::new();
    let mut commons: Vec<PathBuf> = Vec::new();
    // Discover checkout roots, supporting both root/task and root/repo/branch.
    fn inspect(p: &Path, blocked: &mut Vec<(PathBuf, &'static str)>, commons: &mut Vec<PathBuf>) -> std::io::Result<()> {
        let md = std::fs::symlink_metadata(p)?;
        if md.file_type().is_symlink() { return Ok(()); }
        if !md.is_dir() {
            blocked.push((p.to_owned(), "unrecognized file"));
            return Ok(());
        }
        if p.join(".git").exists() {
            match git(p, &["status", "--porcelain", "--untracked-files=all", "--ignored"]) {
                Ok(out) if out.is_empty() => {}
                Ok(_) => blocked.push((p.to_owned(), "uncommitted or ignored files")),
                Err(_) => blocked.push((p.to_owned(), "cannot verify worktree")),
            }
            if let Ok(common) = git(p, &["rev-parse", "--path-format=absolute", "--git-common-dir"]) {
                commons.push(PathBuf::from(common));
            }
            return Ok(());
        }
        for entry in std::fs::read_dir(p)? { inspect(&entry?.path(), blocked, commons)?; }
        Ok(())
    }
    match inspect(&wt_root, &mut blocked, &mut commons) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !wt_root.exists() => {}
        Err(e) => return Err(format!("cannot inspect worktrees: {e}")),
        Ok(()) => {}
    }
    if !blocked.is_empty() && !yes {
        eprintln!("lazed uninstall: refusing to purge checkouts with possible work:");
        for (p, why) in &blocked {
            eprintln!("  {} — {why}", p.display());
        }
        eprintln!("commit/stash that work or rerun with --yes");
        return Err("purge refused; files and daemon retained".into());
    }
    Ok(commons)
}

fn purge(commons: Vec<PathBuf>) -> i32 {
    let wt_root = state::worktrees_dir();
    let mut dirs = vec![state::state_dir(), state::config_dir(), wt_root.clone()];
    dirs.extend(app_data_paths());
    for d in dirs {
        remove_path(&d);
    }
    // ~/.lazed only exists to hold worktrees/ — drop it if left empty (and
    // only when it is literally the .lazed dir, not a custom WT root parent)
    if let Some(parent) = wt_root.parent() {
        if parent.file_name().is_some_and(|n| n == ".lazed") {
            let _ = std::fs::remove_dir(parent);
        }
    }
    for common in commons {
        let _ = std::process::Command::new("git")
            .arg("--git-dir")
            .arg(&common)
            .args(["worktree", "prune"])
            .output();
    }
    println!("note: /Applications/lazed.app is not removed — drag it to Trash yourself");
    0
}

/// Directories lazed owns that plain uninstall keeps and --purge removes.
fn kept_paths() -> Vec<PathBuf> {
    let mut v = vec![
        state::state_dir(),
        state::config_dir(),
        state::worktrees_dir(),
    ];
    v.extend(app_data_paths());
    v
}

fn stop_for_uninstall() -> Result<(), String> {
    use std::os::unix::net::UnixStream;
    use std::io::ErrorKind;
    match std::fs::symlink_metadata(state::sock_path()) {
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("cannot check daemon socket: {e}")),
        Ok(_) => {}
    }
    match UnixStream::connect(state::sock_path()) {
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused) => return Ok(()),
        Err(e) => return Err(format!("cannot check daemon: {e}")),
        Ok(_) => {}
    }
    crate::api_call("server.stop", json!({})).map_err(|e| format!("cannot stop daemon: {e}"))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match UnixStream::connect(state::sock_path()) {
            Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused) => return Ok(()),
            Err(e) => return Err(format!("cannot verify daemon exit: {e}")),
            Ok(_) => {}
        }
        if std::time::Instant::now() >= deadline { return Err("daemon did not exit; uninstall aborted".into()); }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

pub fn uninstall(args: &[String]) -> i32 {
    let mut purge_flag = false;
    let mut yes = false;
    for a in args {
        match a.as_str() {
            "--purge" => purge_flag = true,
            "--yes" | "-y" => yes = true,
            "--help" | "-h" => {
                eprintln!("usage: lazed uninstall [--purge] [--yes]");
                return 0;
            }
            _ => {
                eprintln!("lazed uninstall: unknown option {a}");
                return 2;
            }
        }
    }
    let manifest = read_manifest();
    let found: Vec<PathBuf> = if manifest.is_none() {
        [cli_path()].into_iter().chain(SKILL_LINKS.iter().map(|r| home().join(r)))
            .filter(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink()))
            .collect()
    } else { Vec::new() };
    if !found.is_empty() {
        for p in &found { println!("  remove link {}", p.display()); }
        if !confirm("remove them?", yes) {
            eprintln!("lazed uninstall: aborted");
            return 1;
        }
    }
    let commons = if purge_flag {
        match purge_check(yes) { Ok(v) => v, Err(e) => { eprintln!("{e}"); return 1; } }
    } else { Vec::new() };
    if let Err(e) = stop_for_uninstall() { eprintln!("{e}"); return 1; }
    let mut ok = true;
    if let Some(m) = manifest {
        // remove only links that still point where install put them
        if let Some(list) = m["created"].as_array() {
            for e in list {
                let Some(p) = e["path"].as_str() else { continue };
                let link = PathBuf::from(p);
                match std::fs::symlink_metadata(&link) {
                    Ok(md) if md.file_type().is_symlink() => {
                        let target = e["target"].as_str().map(PathBuf::from);
                        if link_target(&link) == target {
                            match std::fs::remove_file(&link) {
                                Ok(()) => println!("  removed {p}"),
                                Err(e) => {
                                    eprintln!("  could not remove {p}: {e}");
                                    ok = false;
                                }
                            }
                        } else {
                            println!("  left alone {p} (repointed since install)");
                        }
                    }
                    Ok(_) => println!("  left alone {p} (not a symlink)"),
                    Err(_) => {}
                }
            }
        }
        let _ = std::fs::remove_file(manifest_path());
    } else {
        // no manifest (manual/older install): only symlinks at the known
        // paths count as ours — real files/dirs are never touched
        if found.is_empty() {
            println!("no install manifest and no known lazed links found");
        } else {
            println!("no install manifest — found these lazed links:");
            for p in &found {
                let t = std::fs::read_link(p).map(|t| t.display().to_string()).unwrap_or_default();
                println!("  {} -> {t}", p.display());
            }
            for p in &found {
                if let Err(e) = std::fs::remove_file(p) {
                    eprintln!("  could not remove {}: {e}", p.display());
                    ok = false;
                } else {
                    println!("  removed {}", p.display());
                }
            }
        }
    }
    if !ok {
        return 1;
    }
    if purge_flag {
        return purge(commons);
    }
    let kept: Vec<String> = kept_paths()
        .into_iter()
        .filter(|p| p.exists())
        .map(|p| p.display().to_string())
        .collect();
    if !kept.is_empty() {
        println!("kept (rerun with --purge to remove):");
        for p in kept {
            println!("  {p}");
        }
    }
    0
}

fn check(checks: &mut Vec<Value>, id: &str, path: &Path, status: &str, detail: String) {
    checks.push(json!({
        "id": id,
        "path": path.display().to_string(),
        "status": status,
        "detail": detail,
    }));
}

pub fn doctor(args: &[String]) -> i32 {
    let json_out = args.iter().any(|a| a == "--json");
    let exe = self_exe().ok();
    let mut checks: Vec<Value> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    // CLI symlink
    let cli = cli_path();
    let mut cli_target: Option<PathBuf> = None;
    match std::fs::symlink_metadata(&cli) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            missing.push("cli".into());
            check(&mut checks, "cli", &cli, "missing", "run `lazed install`".into());
        }
        Err(e) => check(&mut checks, "cli", &cli, "error", e.to_string()),
        Ok(md) if md.file_type().is_symlink() => {
            cli_target = link_target(&cli);
            let t = cli_target.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
            if cli_target.as_ref().is_some_and(|t| !t.exists()) {
                check(&mut checks, "cli", &cli, "broken", format!("target gone: {t}"));
            } else {
                check(&mut checks, "cli", &cli, "ok", t);
            }
        }
        Ok(_) => check(
            &mut checks,
            "cli",
            &cli,
            "blocked",
            "exists but is not a symlink — not managed by lazed install".into(),
        ),
    }

    // skill links + where a fresh link would point
    let src = exe.as_deref().and_then(skills_source);
    for (id, rel) in [("skill.agents", SKILL_LINKS[0]), ("skill.claude", SKILL_LINKS[1])] {
        let link = home().join(rel);
        match std::fs::symlink_metadata(&link) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                missing.push(id.into());
                check(&mut checks, id, &link, "missing", "run `lazed install`".into());
            }
            Err(e) => check(&mut checks, id, &link, "error", e.to_string()),
            Ok(md) if md.file_type().is_symlink() => match link_target(&link) {
                Some(t) if t.join("SKILL.md").is_file() => {
                    check(&mut checks, id, &link, "ok", t.display().to_string())
                }
                Some(t) => check(&mut checks, id, &link, "broken", format!("no SKILL.md under {}", t.display())),
                None => check(&mut checks, id, &link, "broken", "unreadable target".into()),
            },
            Ok(_) => check(&mut checks, id, &link, "ok", "real directory (not a lazed link)".into()),
        }
    }
    if src.is_none() {
        warnings.push(format!(
            "skill source not found near {} (set LAZED_SKILLS_DIR)",
            exe.as_ref().map(|p| p.display().to_string()).unwrap_or_default()
        ));
    }

    check(
        &mut checks,
        "manifest",
        &manifest_path(),
        if manifest_path().is_file() { "present" } else { "absent" },
        "install manifest".into(),
    );
    for (id, d) in [
        ("state", state::state_dir()),
        ("config", state::config_dir()),
        ("worktrees", state::worktrees_dir()),
    ] {
        check(
            &mut checks,
            id,
            &d,
            if d.is_dir() { "present" } else { "absent" },
            String::new(),
        );
    }

    // daemon: liveness + which binary is actually serving the socket
    let daemon = match crate::api_call("session.status", json!({})) {
        Ok(v) => {
            let dexe = v["exe"].as_str().unwrap_or("").to_string();
            json!({"running": true, "version": v["version"], "exe": dexe, "pid": v["pid"]})
        }
        Err(e) => json!({"running": false, "detail": e}),
    };
    let dexe = daemon["exe"].as_str().unwrap_or("");
    if daemon["running"] == true {
        if let Some(t) = &cli_target {
            if !dexe.is_empty() && Path::new(dexe) != t.as_path() {
                warnings.push(format!(
                    "daemon runs {dexe} but {} links {}",
                    cli.display(),
                    t.display()
                ));
            }
        }
    }

    let app = PathBuf::from("/Applications/lazed.app");
    check(
        &mut checks,
        "app",
        &app,
        if app.is_dir() { "present" } else { "absent" },
        String::new(),
    );

    let agents: Vec<Value> = MANAGED_AGENTS
        .iter()
        .map(|k| {
            let path = std::process::Command::new("which")
                .arg(k)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|p| !p.is_empty());
            match path {
                Some(p) => json!({"kind": k, "path": p}),
                None => json!({"kind": k}),
            }
        })
        .collect();

    let herdr: Vec<String> = HERDR_LEFTOVERS
        .iter()
        .map(|r| home().join(r))
        .filter(|p| p.exists())
        .map(|p| p.display().to_string())
        .collect();

    let failed = checks
        .iter()
        .any(|c| matches!(c["status"].as_str(), Some("missing" | "broken" | "blocked" | "error")));
    let report = json!({
        "ok": !failed,
        "missing": missing,
        "warnings": warnings,
        "checks": checks,
        "daemon": daemon,
        "agents": agents,
        "herdr": herdr,
    });
    if json_out {
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
    } else {
        for c in report["checks"].as_array().into_iter().flatten() {
            let status = c["status"].as_str().unwrap_or("?");
            let mark = match status {
                "ok" | "present" => " ok ",
                "absent" => " -- ",
                _ => "MISS",
            };
            let detail = c["detail"].as_str().unwrap_or("");
            println!(
                "{mark} {:<14} {}{}{}",
                c["id"].as_str().unwrap_or(""),
                c["path"].as_str().unwrap_or(""),
                if detail.is_empty() { "" } else { "  " },
                detail
            );
        }
        println!("daemon   {}", if daemon["running"] == true {
            let exe = daemon["exe"].as_str().unwrap_or("");
            format!("running ({}{})", daemon["version"].as_str().unwrap_or("?"),
                if exe.is_empty() { String::new() } else { format!(" {exe}") })
        } else {
            "not running".into()
        });
        for a in &agents {
            let k = a["kind"].as_str().unwrap_or("");
            match a["path"].as_str() {
                Some(p) => println!(" ok  agent {k:<9} {p}"),
                None => println!(" --  agent {k:<9} not found"),
            }
        }
        if !herdr.is_empty() {
            println!("herdr leftovers (untouched by uninstall): {}", herdr.join(", "));
        }
        for w in report["warnings"].as_array().into_iter().flatten() {
            println!("warning: {}", w.as_str().unwrap_or(""));
        }
    }
    if failed {
        1
    } else {
        0
    }
}
