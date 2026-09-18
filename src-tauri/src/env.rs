//! Process environment for spawned commands.
//!
//! A GUI-launched app inherits a minimal environment — notably a PATH
//! without Homebrew or version-manager dirs — so every shell we spawn
//! merges: the login shell's env (authoritative where it resolved),
//! curated fallback bin dirs (appended, never re-ranking a PATH that did
//! resolve), and stored integration values (only for keys the env
//! doesn't already define — ambient env always wins).

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::integrations;

const ENV_TIMEOUT_SECS: u64 = 6;
const BEGIN_MARK: &str = "__LAZED_ENV_BEGIN__";
const END_MARK: &str = "__LAZED_ENV_END__";

static LOGIN_ENV: OnceLock<HashMap<String, String>> = OnceLock::new();

/// Spawn `cmd` with a deadline. stdout/stderr are drained on threads so a
/// noisy command can't deadlock against a full pipe.
pub fn capture(cmd: &mut Command, timeout_secs: u64) -> Result<String, String> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn failed: {e}"))?;
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let out_t = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout
            .as_mut()
            .map(|o| o.read_to_string(&mut s));
        s
    });
    let err_t = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr
            .as_mut()
            .map(|o| o.read_to_string(&mut s));
        s
    });
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {
                if Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = out_t.join();
                    let _ = err_t.join();
                    return Err(format!("timed out after {timeout_secs}s"));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = out_t.join();
                let _ = err_t.join();
                return Err(format!("wait failed: {e}"));
            }
        }
    };
    let out = out_t.join().unwrap_or_default();
    let err = err_t.join().unwrap_or_default();
    if status.success() {
        Ok(out)
    } else {
        let tail: String = err
            .lines()
            .rev()
            .take(3)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join(" | ");
        Err(if tail.is_empty() {
            format!("exit {}", status)
        } else {
            format!("exit {}: {}", status, tail.chars().take(300).collect::<String>())
        })
    }
}

/// Parse `env` output between markers — `NAME=value` lines start a var,
/// anything else continues the previous value (or is rc-file junk).
fn parse_env(out: &str) -> HashMap<String, String> {
    let mut env = HashMap::new();
    let mut in_block = false;
    let mut last_key: Option<String> = None;
    for line in out.lines() {
        if line.trim() == BEGIN_MARK {
            in_block = true;
            continue;
        }
        if line.trim() == END_MARK {
            break;
        }
        if !in_block {
            continue;
        }
        if let Some(eq) = line.find('=') {
            let (k, v) = line.split_at(eq);
            let valid = k.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            if valid {
                last_key = Some(k.to_string());
                env.insert(k.to_string(), v[1..].to_string());
                continue;
            }
        }
        if let Some(k) = &last_key {
            if let Some(v) = env.get_mut(k) {
                v.push('\n');
                v.push_str(line);
            }
        }
    }
    env
}

/// `$SHELL -lic env` — the user's login env, markers guarding against
/// rc-file noise. `-i` picks up interactive-rc exports (nvm, prompts);
/// `-lc` is the fallback for shells that choke on interactive mode.
fn resolve_login_env() -> HashMap<String, String> {
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/bin/zsh".into());
    let script = format!("echo {BEGIN_MARK}; env; echo {END_MARK}");
    for flags in ["-lic", "-lc"] {
        let mut c = Command::new(&shell);
        c.arg(flags).arg(&script);
        let Ok(out) = capture(&mut c, ENV_TIMEOUT_SECS) else {
            continue;
        };
        let mut env = parse_env(&out);
        // a PATH that absorbed junk continuation lines is worse than none
        if env.get("PATH").is_some_and(|p| p.contains('\n')) {
            env.remove("PATH");
        }
        if env.get("PATH").is_some_and(|p| !p.is_empty()) {
            return env;
        }
    }
    HashMap::new()
}

pub fn login_env() -> &'static HashMap<String, String> {
    LOGIN_ENV.get_or_init(resolve_login_env)
}

/// Resolve the login env off the caller's critical path — pollers and
/// probes hit the warmed cache later.
pub fn warmup() {
    std::thread::spawn(|| {
        login_env();
    });
}

/// Newest installed nvm node's bin dir — the one fallback entry that
/// needs a glob (`~/.nvm/versions/node/*/bin`).
fn nvm_bin_dir() -> Option<PathBuf> {
    let base = PathBuf::from(std::env::var("HOME").ok()?).join(".nvm/versions/node");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(base)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path().join("bin")))
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs.pop()
}

/// Where CLI tools land outside the system PATH — version managers
/// first, system prefixes after. Only existing dirs are appended, so a
/// resolved login PATH is never re-ranked by phantom entries.
fn fallback_bin_dirs() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let h = |p: &str| PathBuf::from(home.clone()).join(p);
    [
        h(".local/bin"),
        h(".local/share/mise/shims"),
        h(".local/share/pnpm"),
        h("Library/pnpm"),
        h(".yarn/bin"),
        h(".bun/bin"),
        h(".volta/bin"),
        h(".asdf/shims"),
        h(".fnm/aliases/default/bin"),
        h(".nix-profile/bin"),
        h(".opencode/bin"),
        h(".vite-plus/bin"),
        h("bin"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/opt/homebrew/sbin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/local/sbin"),
        PathBuf::from("/nix/var/nix/profiles/default/bin"),
    ]
    .into_iter()
    .chain(nvm_bin_dir())
    .filter(|d| d.is_dir())
    .collect()
}

fn merged_path(base: Option<&str>) -> String {
    let mut seen: HashSet<String> = base
        .unwrap_or_default()
        .split(':')
        .map(str::to_string)
        .collect();
    let mut path = base.unwrap_or_default().to_string();
    for d in fallback_bin_dirs() {
        let s = d.to_string_lossy().into_owned();
        if seen.insert(s.clone()) {
            if !path.is_empty() {
                path.push(':');
            }
            path.push_str(&s);
        }
    }
    path
}

/// Env for spawned shells: app env as base, login env overlaid, PATH
/// topped up with fallback dirs, integration values filling gaps.
pub fn env_for_spawn() -> HashMap<String, String> {
    // vars_os + lossy: a non-UTF8 var would panic plain vars() on spawn
    let mut env: HashMap<String, String> = std::env::vars_os()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.to_string_lossy().into_owned(),
            )
        })
        .collect();
    for (k, v) in login_env() {
        env.insert(k.clone(), v.clone());
    }
    let path = merged_path(env.get("PATH").map(String::as_str));
    env.insert("PATH".into(), path);
    for (k, v) in integrations::env_overrides() {
        env.entry(k).or_insert(v);
    }
    env
}

fn is_exec(p: &PathBuf) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// First executable named `cmd` on a PATH — manual walk, so shell
/// aliases/functions can't mask it.
pub fn find_on_path(cmd: &str, path: &str) -> Option<String> {
    if cmd.contains('/') {
        let p = PathBuf::from(cmd);
        return (p.is_file() && is_exec(&p)).then(|| cmd.to_string());
    }
    for dir in path.split(':') {
        if dir.is_empty() {
            continue;
        }
        let p = PathBuf::from(dir).join(cmd);
        if p.is_file() && is_exec(&p) {
            return p.to_string_lossy().into_owned().into();
        }
    }
    None
}

/// Editor's "requires" probe — reports what the spawn env actually sees:
/// each bin resolved (or null) and each env key's presence, whether it
/// comes from the user's env or a stored integration.
pub fn deps_check(input: &Value) -> Value {
    let env = env_for_spawn();
    let path = env.get("PATH").cloned().unwrap_or_default();
    let bins = input
        .get("bins")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(|b| {
                    (
                        b.to_string(),
                        find_on_path(b, &path).map(Value::String).unwrap_or(Value::Null),
                    )
                })
                .collect::<serde_json::Map<String, Value>>()
        })
        .unwrap_or_default();
    let vars = input
        .get("env")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(|k| (k.to_string(), Value::Bool(env.contains_key(k))))
                .collect::<serde_json::Map<String, Value>>()
        })
        .unwrap_or_default();
    serde_json::json!({ "bins": bins, "env": vars })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_env_between_markers_and_skips_junk() {
        let out = "p10k noise\n__LAZED_ENV_BEGIN__\nPATH=/a:/b\nHOME=/u/me\nnot a var\n__LAZED_ENV_END__\ntrailer";
        let env = parse_env(out);
        assert_eq!(env["PATH"], "/a:/b");
        assert_eq!(env["HOME"], "/u/me\nnot a var");
        assert_eq!(env.len(), 2);
    }

    #[test]
    fn merged_path_appends_only_missing() {
        let merged = merged_path(Some("/usr/bin:/bin"));
        for d in merged.split(':').collect::<Vec<_>>() {
            assert!(!d.is_empty());
        }
        let uniq: HashSet<_> = merged.split(':').collect();
        assert_eq!(uniq.len(), merged.split(':').count());
    }
}
