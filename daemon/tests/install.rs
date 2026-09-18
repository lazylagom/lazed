//! install/doctor/uninstall against a throwaway HOME — the real one must
//! never be touched. `CARGO_BIN_EXE_lazed` is the debug binary; its
//! `<exe>/../../../skills/lazed` lookup lands on the repo's skills dir.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

fn lazed(home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_lazed"))
        .args(args)
        .env("HOME", home)
        .env_remove("LAZED_STATE_DIR")
        .env_remove("LAZED_CONFIG_DIR")
        .env_remove("LAZED_WORKTREE_DIR")
        .env_remove("LAZED_SKILLS_DIR")
        .output()
        .unwrap()
}

fn tmp_home(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lazed-install-test-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d.canonicalize().unwrap()
}

fn exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_lazed")).canonicalize().unwrap()
}

fn skills_src() -> PathBuf {
    exe().parent().unwrap().join("../../../skills/lazed").canonicalize().unwrap()
}

/// Every entry under `root` whose name contains "lazed".
fn lazed_traces(root: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let p = e.path();
            if e.file_name().to_string_lossy().contains("lazed") {
                out.push(p.clone());
            }
            if p.is_dir() && !p.is_symlink() {
                lazed_traces(&p, out);
            }
        }
    }
}

fn stderr(o: &std::process::Output) -> String {
    String::from_utf8_lossy(&o.stderr).to_string()
}

#[test]
fn install_doctor_uninstall_purge_leaves_no_trace() {
    let home = tmp_home("full");
    let out = lazed(&home, &["install"]);
    assert!(out.status.success(), "{}", stderr(&out));

    let cli = home.join(".local/bin/lazed");
    assert_eq!(cli.canonicalize().unwrap(), exe());
    for rel in [".agents/skills/lazed", ".claude/skills/lazed"] {
        let link = home.join(rel);
        assert!(link.join("SKILL.md").is_file(), "{rel} should resolve to the skill");
        assert_eq!(link.canonicalize().unwrap(), skills_src());
    }
    assert!(home.join(".local/state/lazed/install.json").is_file());

    // idempotent: a second install is a no-op, not an error
    let out = lazed(&home, &["install"]);
    assert!(out.status.success(), "{}", stderr(&out));

    let out = lazed(&home, &["doctor", "--json"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["ok"], true, "{report}");
    assert!(report["missing"].as_array().unwrap().is_empty());

    let out = lazed(&home, &["uninstall", "--purge", "--yes"]);
    assert!(out.status.success(), "{}", stderr(&out));

    let mut left = Vec::new();
    lazed_traces(&home, &mut left);
    assert!(left.is_empty(), "lazed traces left behind: {left:?}");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn install_never_overwrites_a_real_directory() {
    let home = tmp_home("realdir");
    let real = home.join(".agents/skills/lazed");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::write(real.join("keep.txt"), "mine").unwrap();

    let out = lazed(&home, &["install"]);
    assert!(!out.status.success(), "install must fail on a real dir");
    assert_eq!(std::fs::read_to_string(real.join("keep.txt")).unwrap(), "mine");
    assert!(!real.symlink_metadata().unwrap().file_type().is_symlink());
    // the cli link is independent and still went in
    assert!(home.join(".local/bin/lazed").exists());

    // uninstall removes only what the manifest recorded — the real dir stays
    let out = lazed(&home, &["uninstall", "--yes"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(std::fs::read_to_string(real.join("keep.txt")).unwrap(), "mine");
    assert!(!home.join(".local/bin/lazed").exists());
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn uninstall_without_manifest_removes_only_symlinks() {
    let home = tmp_home("legacy");
    let cli = home.join(".local/bin/lazed");
    let skill = home.join(".agents/skills/lazed");
    std::fs::create_dir_all(cli.parent().unwrap()).unwrap();
    std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(exe(), &cli).unwrap();
    std::os::unix::fs::symlink(skills_src(), &skill).unwrap();

    // non-tty without --yes must refuse rather than guess
    let out = lazed(&home, &["uninstall"]);
    assert!(!out.status.success());
    assert!(cli.exists());

    let out = lazed(&home, &["uninstall", "--yes"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!cli.exists() && !skill.exists());
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn purge_refuses_a_dirty_worktree() {
    check_dirty_worktree("dirtywt", "repo/feat");
}

#[test]
fn purge_refuses_a_dirty_flat_task_worktree_before_uninstalling() {
    check_dirty_worktree("dirtyflat", "task-one");
}

fn check_dirty_worktree(tag: &str, relative: &str) {
    let home = tmp_home(tag);
    assert!(lazed(&home, &["install"]).status.success());
    let repo = home.join("repo");
    let git = |args: &[&str]| {
        let st = Command::new("git").args(args).status().unwrap();
        assert!(st.success());
    };
    git(&["init", "-b", "main", &repo.to_string_lossy()]);
    git(&[
        "-C", &repo.to_string_lossy(),
        "-c", "user.email=t@t", "-c", "user.name=t",
        "commit", "--allow-empty", "-m", "init",
    ]);
    let wt = home.join(".lazed/worktrees").join(relative);
    git(&[
        "-C", &repo.to_string_lossy(),
        "worktree", "add", &wt.to_string_lossy(), "-b", "feat",
    ]);
    std::fs::write(wt.join("dirty.txt"), "uncommitted").unwrap();

    // dirty checkout + no --yes: purge aborts, work stays on disk
    let out = lazed(&home, &["uninstall", "--purge"]);
    assert!(!out.status.success());
    assert!(wt.join("dirty.txt").exists());
    assert!(home.join(".local/bin/lazed").exists());
    assert!(home.join(".local/state/lazed/install.json").exists());

    let out = lazed(&home, &["uninstall", "--purge", "--yes"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!wt.exists());
    let mut left = Vec::new();
    lazed_traces(&home, &mut left);
    assert!(left.is_empty(), "lazed traces left behind: {left:?}");
    let _ = std::fs::remove_dir_all(&home);
}
