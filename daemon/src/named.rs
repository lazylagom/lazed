//! Live agent names, independent of pane/worktree creation and durable tasks.
use crate::{
    control::{self, str_of},
    server::Shared,
    term::lock,
};
use serde_json::{json, Value};

#[derive(Clone)]
pub struct Binding {
    pub term_id: String,
    pub launch_id: String,
    pub starting: bool,
}

fn valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 32
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-".contains(b))
        && !(name.len() > 1
            && name.starts_with('t')
            && name[1..].bytes().all(|b| b.is_ascii_digit()))
}

fn resolve(session: &Shared, target: &str) -> Result<(String, Option<Binding>), String> {
    let binding = lock(session).agent_names.get(target).cloned();
    if let Some(binding) = binding {
        let current = control::handle(session, "agent.get", &json!({"term_id": binding.term_id}));
        if binding.starting
            || current
                .as_ref()
                .is_ok_and(|v| v["launch_id"] == binding.launch_id && v["dead"] != true)
        {
            return Ok((binding.term_id.clone(), Some(binding)));
        }
        let mut s = lock(session);
        if s.agent_names
            .get(target)
            .is_some_and(|b| b.launch_id == binding.launch_id)
        {
            s.agent_names.remove(target);
        }
        return Err(format!(
            "agent_name_expired: {target}; inspect the pane before starting a new agent"
        ));
    }
    if lock(session).terminals.contains_key(target) {
        return Ok((target.into(), None));
    }
    Err(format!("no live agent or pane {target}"))
}

pub fn handle(session: &Shared, method: &str, p: &Value) -> Result<Value, String> {
    if method == "agent.start" && !str_of(p, "name").is_empty() {
        return start(session, p);
    }
    if method == "agent.release" {
        let mut s = lock(session);
        let name = str_of(p, "target");
        let binding = s.agent_names.get(name).ok_or("no such agent name")?;
        if binding.starting {
            return Err("agent_start_pending".into());
        }
        s.agent_names.remove(name);
        return Ok(json!({"released": name, "process_stopped": false}));
    }
    if method == "agent.list" {
        let terms: Vec<String> = lock(session).terminals.keys().cloned().collect();
        let mut agents = Vec::new();
        for tid in terms {
            if let Ok(value) = handle(session, "agent.get", &json!({"term_id": tid})) {
                if value["agent"].is_string() || value["name"].is_string() {
                    agents.push(value);
                }
            }
        }
        return Ok(json!(agents));
    }
    let target = if str_of(p, "target").is_empty() {
        str_of(p, "term_id")
    } else {
        str_of(p, "target")
    };
    let (tid, binding) = resolve(session, target)?;
    let mut params = p.clone();
    params["term_id"] = json!(tid);
    if let Some(binding) = &binding {
        if binding.starting && !matches!(method, "agent.get" | "agent.read") {
            return Err("agent_start_pending: inspect get/read; do not resend start".into());
        }
        if !binding.starting {
            params["launch_id"] = json!(binding.launch_id);
        }
    }
    if method == "agent.wait" && params["after_seq"].is_null() {
        let observed = control::handle(session, "agent.get", &params)?;
        if observed["pending_after"].is_number() {
            params["after_seq"] = observed["pending_after"].clone();
            params["launch_id"] = observed["launch_id"].clone();
        }
    }
    let mut result = control::handle(session, method, &params)?;
    let names: Vec<(String, Binding)> = lock(session)
        .agent_names
        .iter()
        .filter(|(_, b)| b.term_id == tid)
        .map(|(n, b)| (n.clone(), b.clone()))
        .collect();
    for (name, binding) in names {
        if result["launch_id"] == binding.launch_id || binding.starting {
            result["name"] = json!(name);
            result["launch_pending"] = json!(binding.starting);
            break;
        }
    }
    Ok(result)
}

fn start(session: &Shared, p: &Value) -> Result<Value, String> {
    if !str_of(p, "launch_id").is_empty() {
        return Err("named start cannot resume/rebind another launch; inspect agent get/read and wait instead".into());
    }
    let name = str_of(p, "name");
    if !valid_name(name) {
        return Err("agent name must match [a-z][a-z0-9_-]{0,31} and not be a pane ID".into());
    }
    control::resolve(p)?;
    let tid = str_of(p, "term_id");
    let token = control::id();
    let names: Vec<String> = lock(session).agent_names.keys().cloned().collect();
    for existing in names {
        let _ = resolve(session, &existing);
    }
    {
        let mut s = lock(session);
        s.get_terminal(tid)?;
        if s.agent_names.contains_key(name) {
            return Err(format!(
                "agent_name_in_use: {name}; use agent get/read/prompt instead"
            ));
        }
        if s.agent_names.values().any(|b| b.term_id == tid) {
            return Err("pane_already_named: use the existing agent".into());
        }
        s.agent_names.insert(
            name.into(),
            Binding {
                term_id: tid.into(),
                launch_id: token.clone(),
                starting: true,
            },
        );
    }
    let result = control::start_named(session, p, &token);
    let observed = control::handle(session, "agent.get", &json!({"term_id": tid}));
    {
        let mut s = lock(session);
        if !s
            .agent_names
            .get(name)
            .is_some_and(|b| b.launch_id == token)
        {
            return Err("agent_name_lost_during_start".into());
        }
        if observed.as_ref().is_ok_and(|v| v["launch_id"] == token) {
            if let Some(binding) = s.agent_names.get_mut(name) {
                binding.starting = false;
            }
        } else {
            s.agent_names.remove(name);
        }
    }
    match result {
        Ok(mut result) => {
            result["name"] = json!(name);
            Ok(result)
        }
        Err(error) => Err(format!(
            "{error}; name={name}, pane={tid}; inspect agent get/read before retrying"
        )),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn agent_names_are_not_pane_ids_or_ambiguous_labels() {
        for name in ["reviewer", "backend-1", "api_tests"] {
            assert!(super::valid_name(name));
        }
        for name in ["", "t1", "Bad", "two words", "../name"] {
            assert!(!super::valid_name(name));
        }
    }
}
