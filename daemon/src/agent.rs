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
    // interpreted agents run as node/python with the app path in argv
    for (needle, kind) in KNOWN {
        if args.contains(&format!("/{needle}"))
            || args.contains(&format!(" {needle}"))
            || args.ends_with(needle)
        {
            return Some(kind.to_string());
        }
    }
    None
}

/// Claude screen heuristics — reads the bottom of the visible screen.
/// Order matters: explicit prompts beat activity markers.
pub fn claude_status(screen: &str) -> &'static str {
    let lower = screen.to_lowercase();
    // unambiguous modal prompts — no digit check needed
    if lower.contains("trust this folder") || lower.contains("enter to confirm") {
        return "blocked";
    }
    // permission / confirmation prompts — claude renders numbered choices
    let has_yes_choice = lower.contains("1. yes") || lower.contains("❯ 1. yes");
    let asks = lower.contains("do you want to")
        || lower.contains("proceed?")
        || lower.contains("allow")
        || lower.contains("permission")
        || lower.contains("esc to cancel");
    if has_yes_choice || (asks && lower.contains('2')) {
        return "blocked";
    }
    // active work: claude shows a spinner + "esc to interrupt" while running
    if lower.contains("esc to interrupt")
        || lower.contains("thinking")
        || lower.contains("tokens")
    {
        return "working";
    }
    "idle"
}

/// Non-claude agents get coarse status only (no screen grammar yet).
pub fn generic_status(_kind: &str, _screen: &str) -> &'static str {
    "idle"
}

/// Top-level classification: kind + status from process + screen.
pub fn classify(comm: &str, args: &str, screen: &str) -> (Option<String>, String) {
    match classify_kind(comm, args) {
        Some(kind) => {
            let status = if kind == "claude" {
                claude_status(screen).to_string()
            } else {
                generic_status(&kind, screen).to_string()
            };
            (Some(kind), status)
        }
        None => (None, "unknown".to_string()),
    }
}

/// A named agent launch spec from ~/.config/lazed/agents.json.
/// `kind` resolves through the launch-command map, so a spec may point at
/// any argv (e.g. {"kind": "make", "args": ["pi"]} runs `make pi`).
#[derive(serde::Deserialize, Clone)]
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
            None if KNOWN.iter().any(|(_, k)| *k == name) => {
                Ok((name.to_string(), Vec::new()))
            }
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
