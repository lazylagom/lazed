//! One launch/prompt protocol for CLI, GUI and automation. No session lock
//! is held while waiting for a shell, agent, or response.
use crate::{agent::AgentRegistry, server::Shared, term::lock};
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    time::{Duration, Instant},
};

pub fn id() -> String {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .unwrap()
        .read_exact(&mut bytes)
        .unwrap();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub fn str_of<'a>(p: &'a Value, key: &str) -> &'a str {
    p.get(key).and_then(Value::as_str).unwrap_or("")
}
fn timeout(p: &Value, fallback: u64) -> Duration {
    Duration::from_millis(
        p["timeout_ms"]
            .as_u64()
            .unwrap_or(fallback)
            .clamp(100, 600_000),
    )
}

pub fn resolve(p: &Value) -> Result<(String, Vec<String>, String), String> {
    let (kind, mut args) = AgentRegistry::load().resolve(str_of(p, "spec"), str_of(p, "kind"))?;
    if let Some(extra) = p.get("args") {
        for arg in extra.as_array().ok_or("args must be strings")? {
            args.push(arg.as_str().ok_or("args must be strings")?.to_owned());
        }
    }
    let expected = if !str_of(p, "expected_kind").is_empty() {
        str_of(p, "expected_kind").to_owned()
    } else {
        crate::agent::classify_kind(&kind, "")
            .ok_or("wrapper needs expected_kind; use a direct agent spec for managed tasks")?
    };
    if !matches!(expected.as_str(), "claude" | "codex" | "devin" | "pi") {
        return Err(
            "unsupported_readiness: managed launch supports claude, codex, devin and pi".into(),
        );
    }
    if args
        .iter()
        .any(|a| matches!(a.as_str(), "-p" | "--print" | "--mode" | "exec"))
    {
        return Err(
            "interactive launch required; one-shot/exec specs cannot receive agent.prompt".into(),
        );
    }
    Ok((kind, args, expected))
}

pub fn handle(session: &Shared, method: &str, p: &Value) -> Result<Value, String> {
    handle_inner(session, method, p, None)
}

pub fn start_named(session: &Shared, p: &Value, launch: &str) -> Result<Value, String> {
    handle_inner(session, "agent.start", p, Some(launch))
}

fn handle_inner(
    session: &Shared,
    method: &str,
    p: &Value,
    new_launch: Option<&str>,
) -> Result<Value, String> {
    let term = lock(session).get_terminal(str_of(p, "term_id"))?;
    if method == "agent.read" {
        let (text, history) = crate::history::read(&term, p)?;
        let mut g = lock(&term);
        g.detect_agent();
        if !str_of(p, "launch_id").is_empty()
            && g.lifecycle.launch_id.as_deref() != Some(str_of(p, "launch_id"))
        {
            return Err("stale_launch".into());
        }
        let mut result = snapshot(&g);
        result["text"] = json!(text);
        result["history"] = history;
        return Ok(result);
    }
    if method == "agent.get" {
        let mut g = lock(&term);
        g.detect_agent();
        if !str_of(p, "launch_id").is_empty()
            && g.lifecycle.launch_id.as_deref() != Some(str_of(p, "launch_id"))
        {
            return Err("stale_launch".into());
        }
        return Ok(snapshot(&g));
    }
    if method == "agent.report" {
        let mut g = lock(&term);
        if g.lifecycle.launch_id.as_deref() != Some(str_of(p, "launch_id")) {
            return Err("stale_launch".into());
        }
        let seq = p["seq"].as_u64().ok_or("report needs seq")?;
        let state = str_of(p, "state");
        if !matches!(state, "working" | "idle" | "blocked") {
            return Err("invalid state".into());
        }
        if seq <= g.lifecycle.hook_seq {
            return Err("stale_report".into());
        }
        g.detect_agent();
        if g.agent_kind != g.lifecycle.expected_kind || g.agent_kind.is_none() {
            return Err("agent_not_detected".into());
        }
        g.lifecycle.hook_seq = seq;
        g.lifecycle.hook_status = Some(state.into());
        if matches!(state, "working" | "blocked") {
            g.history_reading = false;
        }
        g.detect_agent();
        let result = snapshot(&g);
        drop(g);
        lock(session).broadcast_event("agent.status", result.clone());
        return Ok(result);
    }
    if method == "agent.wait" {
        return wait(session, p, false);
    }
    let control = lock(&term).agent_control.clone();
    let _guard = control.try_lock().map_err(|_| "agent_control_busy")?;
    match method {
        "agent.send_keys" => {
            let keys = p["keys"].as_array().ok_or("keys must be an array")?;
            let bytes = encode_keys(keys)?;
            {
                let mut g = lock(&term);
                g.detect_agent();
                if g.agent_kind.is_none() {
                    return Err("agent_not_detected".into());
                }
                if !str_of(p, "launch_id").is_empty()
                    && g.lifecycle.launch_id.as_deref() != Some(str_of(p, "launch_id"))
                {
                    return Err("stale_launch".into());
                }
            }
            crate::server::term_write(&term, &bytes)?;
            Ok(json!({"term_id": str_of(p, "term_id"), "launch_id": p["launch_id"], "sent": true}))
        }
        "agent.start" => {
            let (kind, mut args, expected) = resolve(p)?;
            let started = Instant::now();
            let budget = timeout(p, 30_000);
            let resume = str_of(p, "launch_id");
            if resume.is_empty() {
                loop {
                    let mut g = lock(&term);
                    g.detect_agent();
                    if g.agent_kind.is_some() {
                        return Err("agent_already_running".into());
                    }
                    if g.is_dead() {
                        return Err("terminal_exited".into());
                    }
                    if g.shell_ready() {
                        break;
                    }
                    if started.elapsed() >= budget {
                        return Err("shell_not_ready: no command sent".into());
                    }
                    drop(g);
                    std::thread::sleep(Duration::from_millis(100));
                }
                let launch = new_launch.map(String::from).unwrap_or_else(id);
                if expected == "pi" {
                    if kind != "pi" && !kind.ends_with("/pi") {
                        return Err("pi wrapper must expose a direct pi executable for lifecycle integration".into());
                    }
                    // A stable, atomically replaced asset is shared by all launches.
                    let asset = crate::state::state_dir().join("lazed-pi.ts");
                    crate::state::atomic_write(&asset, include_bytes!("../integrations/pi.ts"))
                        .map_err(|e| e.to_string())?;
                    args.extend(["--extension".into(), asset.to_string_lossy().into_owned()]);
                }
                let mut argv = vec![crate::server::shell_quote(&kind)];
                argv.extend(args.iter().map(|a| crate::server::shell_quote(a)));
                let command = format!("env LAZED_LAUNCH_ID={launch} {}\r", argv.join(" "));
                {
                    let mut g = lock(&term);
                    g.lifecycle = crate::agent::Lifecycle {
                        launch_id: Some(launch),
                        expected_kind: Some(expected.clone()),
                        ..Default::default()
                    };
                }
                crate::server::term_write(&term, command.as_bytes())
                    .map_err(|e| format!("launch_uncertain: {e}"))?;
            } else if lock(&term).lifecycle.launch_id.as_deref() != Some(resume) {
                return Err("stale_launch".into());
            }
            let mut ready_since = None;
            loop {
                let mut g = lock(&term);
                g.detect_agent();
                if g.is_dead() {
                    return Err("terminal_exited".into());
                }
                if g.agent_kind.as_deref() == Some(&expected) {
                    if g.agent_status == "blocked" {
                        return Err("agent_blocked: inspect the pane; approve manually, then resume this launch".into());
                    }
                    if matches!(g.agent_status.as_str(), "idle" | "done") {
                        let since = ready_since.get_or_insert_with(Instant::now);
                        if g.bracketed_paste() && since.elapsed() >= Duration::from_millis(600) {
                            return Ok(snapshot(&g));
                        }
                    } else {
                        ready_since = None;
                    }
                } else {
                    ready_since = None;
                }
                if started.elapsed() >= budget {
                    return Err(
                        "agent_ready_timeout: launch retained; inspect before retrying".into(),
                    );
                }
                drop(g);
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        "agent.prompt" => {
            let started = Instant::now();
            let budget = timeout(p, 30_000);
            let text = validate_text(str_of(p, "text"))?;
            let (launch, baseline, paste, writer, was_working) = {
                let mut g = lock(&term);
                g.detect_agent();
                if g.is_dead() || g.agent_kind.is_none() {
                    return Err("agent_not_detected".into());
                }
                if !matches!(g.agent_status.as_str(), "idle" | "done" | "working") {
                    return Err(format!("agent_not_ready: {}", g.agent_status));
                }
                let was_working = g.agent_status == "working";
                if g.lifecycle.pending_after.is_some() && !was_working {
                    return Err("submission_pending: previous prompt has not settled; inspect before sending anything else".into());
                }
                if g.lifecycle.launch_id.is_none() {
                    g.lifecycle.launch_id = Some(id());
                    g.lifecycle.expected_kind = g.agent_kind.clone();
                }
                if !str_of(p, "launch_id").is_empty()
                    && g.lifecycle.launch_id.as_deref() != Some(str_of(p, "launch_id"))
                {
                    return Err("stale_launch".into());
                }
                if text.contains('\n') && !g.bracketed_paste() {
                    return Err("multiline_requires_bracketed_paste: no text sent".into());
                }
                g.scroll_to(0);
                g.user_scrolled = false;
                // Herdr-style lifecycle wait, NOT a per-prompt completion receipt.
                // A busy agent already has activity: its current turn may settle
                // before the native agent consumes this follow-up.
                let baseline = if was_working {
                    g.lifecycle.activity_seq.saturating_sub(1)
                } else {
                    g.lifecycle.state_seq
                };
                g.lifecycle.pending_after = Some(baseline);
                (
                    g.lifecycle.launch_id.clone(),
                    baseline,
                    g.bracketed_paste(),
                    g.writer(),
                    was_working,
                )
            };
            let bytes = if paste {
                format!("\x1b[200~{text}\x1b[201~")
            } else {
                text
            };
            {
                let mut w = lock(&writer);
                w.write_all(bytes.as_bytes())
                    .and_then(|_| w.flush())
                    .map_err(|e| format!("submission_uncertain: {e}"))?;
                // Paste must be consumed before Enter. Keep the writer locked
                // across both writes, so other input cannot interleave them.
                std::thread::sleep(Duration::from_millis(80));
                w.write_all(b"\r")
                    .and_then(|_| w.flush())
                    .map_err(|e| format!("submission_uncertain: {e}"))?;
            }
            // A waiting prompt must not prevent a follow-up or deliberate Esc.
            // Only the ordered paste+Enter submission is serialized.
            drop(_guard);
            let activity_ms = if p["wait"] == true {
                budget
                    .saturating_sub(started.elapsed())
                    .as_millis()
                    .min(5000) as u64
            } else {
                p["timeout_ms"].as_u64().unwrap_or(10_000)
            };
            let mut receipt = json!({"term_id": str_of(p, "term_id"), "launch_id": launch, "after_seq": baseline, "timeout_ms": activity_ms,
                "submitted": true, "started_while_working": was_working, "wait_scope": "agent_lifecycle"});
            match wait(session, &receipt, true) {
                Ok(observed) => {
                    receipt["observed"] = observed;
                    receipt["accepted"] = json!(true);
                }
                Err(error) => {
                    receipt["accepted"] = Value::Null;
                    receipt["error"] =
                        json!(format!("submission_uncertain: {error}; do not resend"));
                }
            }
            if p["wait"] == true && receipt["accepted"] == true {
                let remaining = budget.saturating_sub(started.elapsed());
                receipt["timeout_ms"] = json!(remaining.as_millis() as u64);
                let outcome = if remaining.is_zero() {
                    Err("agent_wait_timeout".into())
                } else {
                    wait(session, &receipt, false)
                };
                match outcome {
                    Ok(observed) => {
                        receipt["observed"] = observed;
                    }
                    Err(error) => {
                        receipt["error"] = json!(format!(
                            "{error}; prompt was submitted; inspect before retrying"
                        ));
                    }
                }
            }
            Ok(receipt)
        }
        _ => Err(format!("unknown control method {method}")),
    }
}

fn encode_keys(keys: &[Value]) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    for key in keys {
        let key = key.as_str().ok_or("key must be a string")?;
        let encoded = match key {
            "enter" | "return" => "\r",
            "esc" | "escape" => "\x1b",
            "tab" => "\t",
            "ctrl+c" => "\x03",
            "ctrl+d" => "\x04",
            "ctrl+u" => "\x15",
            "backspace" => "\x7f",
            "space" => " ",
            "up" => "\x1b[A",
            "down" => "\x1b[B",
            "right" => "\x1b[C",
            "left" => "\x1b[D",
            one if one.chars().count() == 1 && !one.chars().any(char::is_control) => one,
            _ => return Err(format!("unknown logical key {key}; no input sent")),
        };
        bytes.extend_from_slice(encoded.as_bytes());
    }
    Ok(bytes)
}

pub fn validate_text(text: &str) -> Result<String, String> {
    let text = text.replace("\r\n", "\n");
    if text.trim().is_empty() || text.len() > 65536 {
        return Err("prompt must contain 1..65536 bytes".into());
    }
    if text
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err("prompt contains terminal control characters".into());
    }
    Ok(text)
}
fn snapshot(g: &crate::term::PtyTerm) -> Value {
    json!({"term_id": g.id, "agent": g.agent_kind, "agent_status": g.agent_status,
        "launch_id": g.lifecycle.launch_id, "state_seq": g.lifecycle.state_seq,
        "activity_seq": g.lifecycle.activity_seq, "source": g.lifecycle.source,
        "pending_after": g.lifecycle.pending_after, "dead": g.is_dead()})
}
fn wait(session: &Shared, p: &Value, activity_only: bool) -> Result<Value, String> {
    let term = lock(session).get_terminal(str_of(p, "term_id"))?;
    let start = Instant::now();
    let after = p["after_seq"].as_u64();
    if after.is_some() && str_of(p, "launch_id").is_empty() {
        return Err("after_seq requires launch_id".into());
    }
    loop {
        let mut g = lock(&term);
        g.detect_agent();
        if !str_of(p, "launch_id").is_empty()
            && g.lifecycle.launch_id.as_deref() != Some(str_of(p, "launch_id"))
        {
            return Err("stale_launch".into());
        }
        if g.is_dead() || g.agent_kind.is_none() {
            return Err("agent_exited".into());
        }
        let active = after.is_none_or(|seq| g.lifecycle.activity_seq > seq);
        let matches = p["until"]
            .as_array()
            .map(|a| {
                a.iter().any(|v| {
                    v.as_str() == Some(&g.agent_status) || (v == "idle" && g.agent_status == "done")
                })
            })
            .unwrap_or(matches!(
                g.agent_status.as_str(),
                "idle" | "done" | "blocked"
            ));
        if active && (activity_only || matches) {
            return Ok(snapshot(&g));
        }
        if start.elapsed() >= timeout(p, 30_000) {
            return Err("agent_wait_timeout".into());
        }
        drop(g);
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn logical_keys_validate_all_input_before_writing() {
        assert_eq!(
            super::encode_keys(
                &serde_json::json!(["1", "enter", "ctrl+c"])
                    .as_array()
                    .unwrap()
            )
            .unwrap(),
            b"1\r\x03"
        );
        assert!(super::encode_keys(
            &serde_json::json!(["enter", "not-a-key"])
                .as_array()
                .unwrap()
        )
        .is_err());
    }
    #[test]
    fn prompt_validation() {
        assert!(super::validate_text("한글\n'quoted'\ttext").is_ok());
        assert!(super::validate_text("\x1b[201~rm").is_err());
        assert!(super::validate_text("\r").is_err());
        assert!(super::validate_text("").is_err());
    }
}
