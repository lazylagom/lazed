//! Agent launch specs — ~/.config/lazed/agents.json. A spec names a herdr
//! agent kind plus default argv, so `lazed task start --spec luna` can
//! stand for `--kind pi -- --model gpt-5.6-luna`. Detection, readiness and
//! lifecycle are herdr's (manifests); this is only the launch shorthand.

/// Agent kinds herdr ships manifests for (0.9.0). An unregistered spec
/// name that is itself a kind passes through unchanged.
pub const KNOWN_KINDS: &[&str] = &[
    "agy", "amp", "claude", "cline", "codex", "copilot", "cursor", "devin", "droid", "gemini",
    "grok", "hermes", "kilo", "kimi", "kiro", "maki", "muse", "opencode", "pi", "qodercli", "qwen",
];

/// A named agent launch spec from ~/.config/lazed/agents.json.
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
                .ok_or("agent start needs kind or spec (no default in agents.json)")?
        };
        match self.specs.get(name) {
            Some(sp) => Ok((sp.kind.clone(), sp.args.clone())),
            None if KNOWN_KINDS.contains(&name) => Ok((name.to_string(), Vec::new())),
            None => Err(format!(
                "unknown agent spec '{name}' (specs: {}; or a known kind)",
                self.names().join(", ")
            )),
        }
    }

    pub fn json(&self) -> serde_json::Value {
        let specs: serde_json::Map<String, serde_json::Value> = self
            .specs
            .iter()
            .map(|(n, sp)| (n.clone(), serde_json::json!({"kind": sp.kind, "args": sp.args})))
            .collect();
        serde_json::json!({"default": self.default, "specs": specs, "kinds": KNOWN_KINDS})
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
            AgentSpec { kind: "pi".into(), args: vec!["--model".into(), "gpt-5.6-luna".into()] },
        );
        specs.insert("pi".into(), AgentSpec { kind: "pi".into(), args: vec![] });
        AgentRegistry { default: Some("pi".into()), specs }
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
        assert_eq!(reg().resolve("codex", "").unwrap().0, "codex");
        assert_eq!(reg().resolve("luna", "claude").unwrap().0, "pi");
    }

    #[test]
    fn resolve_errors() {
        assert!(reg().resolve("nope", "").is_err());
        assert!(AgentRegistry::default().resolve("", "").is_err());
    }
}
