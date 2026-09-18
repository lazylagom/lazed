//! Agent detection: foreground process identification + screen heuristics.
//!
//! Statuses: "working" | "blocked" | "done" | "idle" | "unknown".
//! Detection runs on a tick (session agent_watch thread); the foreground
//! process comes from the PTY's process group leader, and claude's state
//! is read off the bottom of the visible screen.

/// Agent kinds we recognize by executable name or argv match.
const KNOWN: &[(&str, &str)] = &[
    ("claude", "claude"),
    ("codex", "codex"),
    ("antigravity", "antigravity"),
    ("devin", "devin"),
    ("gemini", "gemini"),
    ("pi", "pi"),
    ("aider", "aider"),
    ("opencode", "opencode"),
];

/// Classify a foreground command line into an agent kind.
/// `comm` is the executable basename, `args` the full command line.
pub fn classify_kind(comm: &str, args: &str) -> Option<String> {
    let base = std::path::Path::new(comm)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| comm.to_string());
    for (needle, kind) in KNOWN {
        if base == *needle {
            return Some(kind.to_string());
        }
    }
    // Inspect executable/script tokens, never arbitrary prompt text in a shell.
    if ["node", "bun", "python", "python3"]
        .iter()
        .any(|p| base.to_lowercase().starts_with(p))
    {
        for arg in args.split_whitespace().skip(1).take(2) {
            let name = std::path::Path::new(arg).file_name()?.to_str()?;
            for (needle, kind) in KNOWN {
                if name == *needle
                    || name == format!("{needle}.js")
                    || (*kind == "pi" && arg.contains("/pi-coding-agent/") && name == "cli.js")
                {
                    return Some(kind.to_string());
                }
            }
        }
    }
    None
}

/// Claude screen heuristics — reads the bottom of the visible screen.
/// Order matters: explicit prompts beat activity markers.
pub fn claude_status(screen: &str) -> &'static str {
    generic_status("claude", screen)
}

/// Devin screen heuristics — its TUI markers differ from claude/codex:
/// working shows "esc twice to interrupt", the input prompt is ❭, and
/// approval/question walls render their own dialog strings. The ❭ input
/// stays visible while working, so activity markers must win over idle.
pub fn devin_status(screen: &str) -> &'static str {
    let lower = screen.to_lowercase();
    if lower.contains("tool approval")
        || lower.contains("devin needs input")
        || lower.contains("devin needs authentication")
        || lower.contains("authentication required")
        || lower.contains("(allow once)")
        || lower.contains("press q to return")
    {
        return "blocked";
    }
    if lower.contains("esc twice to interrupt") || lower.contains("esc again to interrupt") {
        return "working";
    }
    if screen.lines().any(|l| l.trim_start().starts_with('❭')) {
        return "idle";
    }
    "unknown"
}

/// Only positive UI evidence is a state. Unknown screen shapes stay unknown.
pub fn generic_status(kind: &str, screen: &str) -> &'static str {
    let lower = screen.to_lowercase();
    if lower.contains("trust this folder")
        || lower.contains("trust the files")
        || lower.contains("trust this directory")
        || ((lower.contains("1. yes") || lower.contains("enter to confirm"))
            && (lower.contains("allow")
                || lower.contains("proceed")
                || lower.contains("trust")
                || lower.contains("permission")))
    {
        return "blocked";
    }
    if lower.contains("esc to interrupt")
        || lower.contains("escape to interrupt")
        || (kind == "pi" && lower.contains("esc to cancel"))
    {
        return "working";
    }
    let prompt = screen.lines().any(|l| {
        let s = l.trim_start();
        s.starts_with('❯') || s.starts_with('›')
    });
    if prompt && matches!(kind, "claude" | "codex") {
        return "idle";
    }
    "unknown"
}

/// A new launch invalidates all reports and activity from the old occupant.
#[derive(Default)]
pub struct Lifecycle {
    pub launch_id: Option<String>,
    pub process_group: Option<i32>,
    pub expected_kind: Option<String>,
    pub state_seq: u64,
    pub activity_seq: u64,
    pub pending_after: Option<u64>,
    pub hook_seq: u64,
    pub hook_status: Option<String>,
    pub source: String,
}

impl Lifecycle {
    pub fn record(&mut self, status: &str) {
        self.state_seq += 1;
        if matches!(status, "working" | "blocked") {
            self.activity_seq = self.state_seq;
        }
    }
}

/// Top-level classification: kind + status from process + screen.
pub fn classify(comm: &str, args: &str, screen: &str) -> (Option<String>, String) {
    match classify_kind(comm, args) {
        Some(kind) => {
            let status = match kind.as_str() {
                "claude" => claude_status(screen).to_string(),
                "devin" => devin_status(screen).to_string(),
                _ => generic_status(&kind, screen).to_string(),
            };
            (Some(kind), status)
        }
        None => (None, "unknown".to_string()),
    }
}

/// A named agent launch spec from ~/.config/lazed/agents.json.
/// `kind` resolves through the launch-command map, so a spec may point at
/// any argv (e.g. {"kind": "make", "args": ["pi"]} runs `make pi`).
#[derive(serde::Deserialize, serde::Serialize, Clone)]
pub struct AgentSpec {
    pub kind: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(serde::Deserialize, Default)]
pub struct AgentRegistry {
    pub default: Option<String>,
    #[serde(default)]
    pub specs: std::collections::HashMap<String, AgentSpec>,
}

impl AgentRegistry {
    /// Read ~/.config/lazed/agents.json; missing/invalid file → empty.
    pub fn load() -> Self {
        std::fs::read_to_string(crate::state::agents_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.specs.keys().cloned().collect();
        v.sort();
        v
    }

    /// Resolve a launch request: an explicit spec name wins; with neither
    /// spec nor kind the registry default applies; a bare kind passes
    /// through unchanged. Returns (kind, spec args) — caller appends any
    /// per-call args after these.
    pub fn resolve(&self, spec: &str, kind: &str) -> Result<(String, Vec<String>), String> {
        if spec.is_empty() && !kind.is_empty() {
            return Ok((kind.to_string(), Vec::new()));
        }
        let name = if !spec.is_empty() {
            spec
        } else {
            self.default
                .as_deref()
                .ok_or("agent.start needs kind or spec (no default in agents.json)")?
        };
        match self.specs.get(name) {
            Some(sp) => Ok((sp.kind.clone(), sp.args.clone())),
            // an unregistered name that is itself a known kind works too
            None if KNOWN.iter().any(|(_, k)| *k == name) => Ok((name.to_string(), Vec::new())),
            None => Err(format!(
                "unknown agent spec '{name}' (specs: {}; or a known kind)",
                self.names().join(", ")
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentRegistry, AgentSpec};
    use std::collections::HashMap;

    fn reg() -> AgentRegistry {
        let mut specs = HashMap::new();
        specs.insert(
            "luna".into(),
            AgentSpec {
                kind: "pi".into(),
                args: vec!["--model".into(), "gpt-5.6-luna".into()],
            },
        );
        specs.insert(
            "pi".into(),
            AgentSpec {
                kind: "pi".into(),
                args: vec![],
            },
        );
        AgentRegistry {
            default: Some("pi".into()),
            specs,
        }
    }

    #[test]
    fn resolve_spec_and_kind() {
        let (kind, args) = reg().resolve("luna", "").unwrap();
        assert_eq!(kind, "pi");
        assert_eq!(args, ["--model", "gpt-5.6-luna"]);

        let (kind, args) = reg().resolve("", "claude").unwrap();
        assert_eq!(kind, "claude");
        assert!(args.is_empty());
    }

    #[test]
    fn resolve_default_and_fallback() {
        assert_eq!(reg().resolve("", "").unwrap().0, "pi");
        // unregistered but a known kind → bare kind
        assert_eq!(reg().resolve("codex", "").unwrap().0, "codex");
        // spec beats kind when both given
        assert_eq!(reg().resolve("luna", "claude").unwrap().0, "pi");
    }

    #[test]
    fn resolve_errors() {
        assert!(reg().resolve("nope", "").is_err());
        let empty = AgentRegistry::default();
        assert!(empty.resolve("", "").is_err());
    }

    #[test]
    fn status_requires_live_ui_evidence() {
        assert_eq!(
            super::claude_status("Churned for 3m · 123 tokens\n❯ "),
            "idle"
        );
        assert_eq!(
            super::claude_status("Thinking about tokens in an article"),
            "unknown"
        );
        assert_eq!(
            super::generic_status("codex", "Working (esc to interrupt)\n› "),
            "working"
        );
        assert_eq!(super::generic_status("pi", "some output"), "unknown");
        assert_eq!(
            super::claude_status("Do you trust this folder?\n1. Yes"),
            "blocked"
        );
        assert_eq!(super::classify_kind("zsh", "zsh -c echo codex"), None);
        assert_eq!(
            super::classify_kind("node", "node /pkg/pi-coding-agent/dist/bundle/cli.js"),
            Some("pi".into())
        );
    }

    #[test]
    fn devin_status_uses_its_own_markers() {
        // Working spinner: the ❭ input is visible too, so activity must win.
        assert_eq!(
            super::devin_status(
                "⡆⠀ Running tools · 36s (esc twice to interrupt)\n❭ Guide Devin while it works"
            ),
            "working"
        );
        assert_eq!(
            super::devin_status("Running tools · 3s (esc again to interrupt)"),
            "working"
        );
        // Idle input box.
        assert_eq!(
            super::devin_status("❭ Ask Devin to build features, fix bugs, or work on your code"),
            "idle"
        );
        // Approval/question/auth walls.
        assert_eq!(
            super::devin_status("Tool approval\n(Allow once) (This session only)"),
            "blocked"
        );
        assert_eq!(super::devin_status("Devin needs input"), "blocked");
        assert_eq!(super::devin_status("Authentication required"), "blocked");
        assert_eq!(
            super::devin_status("Tool approval pending - press q to return"),
            "blocked"
        );
        // The persistent mode banner is not a dialog.
        assert_eq!(
            super::devin_status("─── (bypass permissions on) ─"),
            "unknown"
        );
        assert_eq!(super::devin_status("some transcript output"), "unknown");
        // classify routes devin screens through devin_status.
        assert_eq!(
            super::classify("devin", "devin", "❭ Ask Devin anything").1,
            "idle"
        );
    }

    #[test]
    fn activity_survives_a_fast_settled_transition() {
        let mut state = super::Lifecycle::default();
        state.record("idle");
        let baseline = state.state_seq;
        state.record("working");
        state.record("idle");
        assert!(state.activity_seq > baseline);
    }
}

/// Resolve the foreground process of a PTY: the process group leader pid,
/// then its comm + full command line via ps(1).
pub fn fg_process(pgid: i32) -> Option<(String, String)> {
    let out = std::process::Command::new("ps")
        .args(["-o", "comm=", "-p", &pgid.to_string()])
        .output()
        .ok()?;
    let comm = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let out2 = std::process::Command::new("ps")
        .args(["-o", "command=", "-p", &pgid.to_string()])
        .output()
        .ok()?;
    let args = String::from_utf8_lossy(&out2.stdout).trim().to_string();
    if comm.is_empty() && args.is_empty() {
        return None;
    }
    Some((comm, args))
}

/// Batch `fg_process`: one ps(1) spawn per column for the whole pgid set.
/// The agent watcher calls this once per tick instead of spawning ps
/// twice per terminal — pids absent from the output (dead between the
/// pgid read and the spawn) are simply missing from the map.
pub fn fg_processes(pgids: &[i32]) -> std::collections::HashMap<i32, (String, String)> {
    let mut map = std::collections::HashMap::new();
    if pgids.is_empty() {
        return map;
    }
    let ids = pgids
        .iter()
        .map(|p| p.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let column = |fmt: &str| -> std::collections::HashMap<i32, String> {
        let Ok(out) = std::process::Command::new("ps")
            .args(["-o", "pid=", "-o", fmt, "-p", &ids])
            .output()
        else {
            return std::collections::HashMap::new();
        };
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| {
                let l = l.trim_start();
                let sp = l.find(char::is_whitespace)?;
                Some((l[..sp].parse().ok()?, l[sp..].trim().to_string()))
            })
            .collect()
    };
    let comms = column("comm=");
    let cmds = column("command=");
    for (pid, comm) in comms {
        let args = cmds.get(&pid).cloned().unwrap_or_default();
        map.insert(pid, (comm, args));
    }
    map
}
