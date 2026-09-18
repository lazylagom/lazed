//! Bounded alternate-screen transcript reads. Ordinary reads stay passive;
//! supported idle agents can be scrolled without changing lifecycle evidence.
use crate::{
    control::str_of,
    term::{lock, PtyTerm},
};
use serde_json::{json, Value};
use std::{
    io::Write,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub text: String,
    pub wrapped: bool,
}

type Term = Arc<Mutex<PtyTerm>>;

struct ReadGuard(Term);
impl Drop for ReadGuard {
    fn drop(&mut self) {
        lock(&self.0).history_reading = false;
    }
}

pub fn read(term: &Term, p: &Value) -> Result<(String, Value), String> {
    let source = str_of(p, "source");
    let lines = p["lines"].as_u64().unwrap_or(120).clamp(1, 2000) as usize;
    let (fallback, eligible, alt, control) = {
        let mut g = lock(term);
        g.detect_agent();
        if !str_of(p, "launch_id").is_empty()
            && g.lifecycle.launch_id.as_deref() != Some(str_of(p, "launch_id"))
        {
            return Err("stale_launch".into());
        }
        let fallback = g.read_text(source, lines)?;
        let alt = g.alternate_screen();
        let eligible = matches!(source, "" | "recent" | "recent-unwrapped")
            && lines > g.rows()
            && g.history_mouse_supported()
            && !g.user_scrolled
            && g.lifecycle.launch_id.is_some()
            && g.lifecycle.pending_after.is_none()
            && matches!(g.agent_kind.as_deref(), Some("claude" | "codex" | "pi"));
        (fallback, eligible, alt, g.agent_control.clone())
    };
    if !eligible {
        return Ok((
            fallback,
            json!({"method": "passive", "viewport_restored": true,
            "alternate_screen": alt, "note": if alt { "Only the available viewport is returned; history may be incomplete." } else { "Host screen and scrollback." }}),
        ));
    }
    let _control = control
        .try_lock()
        .map_err(|_| "agent_control_busy: retry read after input settles")?;
    let (initial, input_seq, launch, cols) = {
        let mut g = lock(term);
        g.detect_agent();
        if !matches!(g.agent_status.as_str(), "idle" | "done") {
            return Err("agent_not_idle: use --source visible while working or blocked".into());
        }
        if !g.history_mouse_supported()
            || g.user_scrolled
            || g.is_dead()
            || g.lifecycle.pending_after.is_some()
        {
            return Ok((
                fallback,
                json!({"method": "passive", "note": "Viewport changed before history read."}),
            ));
        }
        if !str_of(p, "launch_id").is_empty()
            && g.lifecycle.launch_id.as_deref() != Some(str_of(p, "launch_id"))
        {
            return Err("stale_launch".into());
        }
        g.history_reading = true;
        (
            g.history_rows(),
            g.input_seq,
            g.lifecycle.launch_id.clone(),
            g.cols(),
        )
    };
    let _reading = ReadGuard(term.clone());
    let step = |down: bool| wheel(term, down, input_seq, &launch, cols);
    let started = Instant::now();
    // Do not assume an application's own viewport is at the bottom. A downward
    // probe must leave it unchanged, otherwise undo and return passive output.
    let bottom = match step(true) {
        Ok(rows) => rows,
        Err(reason) => {
            return Ok((
                fallback,
                json!({"method": "passive", "note": reason, "viewport_restored": false}),
            ))
        }
    };
    if bottom != initial {
        let restored = step(false).is_ok_and(|rows| rows == initial);
        return Ok((
            fallback,
            json!({"method": "passive", "note": "Not at a stable transcript bottom; history was not harvested.", "viewport_restored": restored}),
        ));
    }
    let mut history = initial.clone();
    let mut previous = initial.clone();
    let mut upward = 0;
    let mut reached_top = false;
    let mut note = None;
    while history.len() < lines && upward < 120 && started.elapsed() < Duration::from_secs(12) {
        upward += 1;
        let next = match step(false) {
            Ok(rows) => rows,
            Err(e) => {
                note = Some(e);
                break;
            }
        };
        if next == previous {
            reached_top = true;
            break;
        }
        if !merge(&mut history, &previous, &next) {
            note = Some("ambiguous transcript overlap; returning only verified rows".into());
            break;
        }
        previous = next;
    }
    // Restore before returning, including on timeout/ambiguous overlap. Stop
    // immediately if a human inputs, resizes, or replaces the agent.
    let restore_started = Instant::now();
    let mut restored = false;
    for _ in 0..upward + 2 {
        if restore_started.elapsed() >= Duration::from_secs(5) {
            break;
        }
        match step(true) {
            Ok(rows) if rows == initial => {
                restored = true;
                break;
            }
            Ok(_) => {}
            Err(e) => {
                note = Some(e);
                break;
            }
        }
    }
    if !restored {
        return Ok((
            fallback,
            json!({"method": "passive", "viewport_restored": false,
            "note": note.unwrap_or_else(|| "Could not verify viewport restoration; inspect the pane.".into())}),
        ));
    }
    let text = render(&history, lines, source != "recent");
    Ok((
        text,
        json!({"method": "alternate-screen", "viewport_restored": true,
        "rows": history.len(), "reached_top": reached_top, "limited": !reached_top,
        "note": note}),
    ))
}

fn wheel(
    term: &Term,
    down: bool,
    input_seq: u64,
    launch: &Option<String>,
    cols: usize,
) -> Result<Vec<Row>, String> {
    let writer = lock(term).writer();
    {
        let mut writer = lock(&writer);
        let mut g = lock(term);
        g.detect_agent();
        if g.is_dead()
            || g.input_seq != input_seq
            || !g.history_reading
            || &g.lifecycle.launch_id != launch
            || g.cols() != cols
            || !g.history_mouse_supported()
        {
            return Err("history read cancelled by input, resize, or agent change".into());
        }
        let button = if down { 65 } else { 64 };
        let bytes = format!(
            "\x1b[<{button};{};{}M",
            (cols / 2).max(1),
            (g.rows() / 2).max(1)
        )
        .repeat(3);
        drop(g);
        writer
            .write_all(bytes.as_bytes())
            .and_then(|_| writer.flush())
            .map_err(|e| e.to_string())?;
    }
    // Wait for redraw to settle, bounded even for animated/unresponsive UIs.
    let started = Instant::now();
    let mut last_seq = lock(term).output_seq;
    let mut quiet = Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(20));
        let g = lock(term);
        if g.input_seq != input_seq
            || !g.history_reading
            || &g.lifecycle.launch_id != launch
            || g.is_dead()
        {
            return Err("history read cancelled by input or agent change".into());
        }
        if g.output_seq != last_seq {
            last_seq = g.output_seq;
            quiet = Instant::now();
        }
        if started.elapsed() >= Duration::from_millis(120)
            && quiet.elapsed() >= Duration::from_millis(60)
        {
            return Ok(g.history_rows());
        }
        if started.elapsed() >= Duration::from_millis(400) {
            return Err("history redraw did not settle".into());
        }
    }
}

// Find a unique shifted overlap, tolerating pinned headers and footers. Two
// distinct anchor rows are required: blank/repeated lines cannot prove order.
fn merge(history: &mut Vec<Row>, previous: &[Row], next: &[Row]) -> bool {
    if previous.len() != next.len() {
        return false;
    }
    let mut candidates = Vec::new();
    for shift in 1..previous.len() {
        let anchors: Vec<usize> = (0..previous.len() - shift)
            .filter(|&i| {
                let row = &previous[i];
                !row.text.trim().is_empty()
                    && row == &next[i + shift]
                    && previous.iter().filter(|r| *r == row).count() == 1
                    && next.iter().filter(|r| *r == row).count() == 1
            })
            .collect();
        if anchors.len() >= 2 {
            candidates.push((shift, anchors[0]));
        }
    }
    if candidates.len() != 1 {
        return false;
    }
    let (shift, anchor) = candidates[0];
    let Some(at) = history
        .iter()
        .take(previous.len())
        .position(|r| r == &previous[anchor])
    else {
        return false;
    };
    history.splice(..at, next[..anchor + shift].iter().cloned());
    true
}

fn render(rows: &[Row], limit: usize, unwrap: bool) -> String {
    let mut text = String::new();
    for row in &rows[rows.len().saturating_sub(limit)..] {
        text.push_str(&row.text);
        if !unwrap || !row.wrapped {
            text.push('\n');
        }
    }
    text.trim_end_matches('\n').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rows(values: &[&str]) -> Vec<Row> {
        values
            .iter()
            .map(|s| Row {
                text: (*s).into(),
                wrapped: false,
            })
            .collect()
    }
    #[test]
    fn merge_pinned_chrome_and_reject_ambiguous_history() {
        let old = rows(&["header", "c", "d", "e", "f", "prompt"]);
        let new = rows(&["header", "a", "b", "c", "d", "prompt"]);
        let mut history = old.clone();
        assert!(merge(&mut history, &old, &new));
        assert_eq!(
            render(&history, 20, false),
            "header\na\nb\nc\nd\ne\nf\nprompt"
        );
        let repeated = rows(&["header", "x", "x", "x", "x", "prompt"]);
        assert!(!merge(&mut history, &old, &repeated));
    }
    #[test]
    fn unwrap_preserves_soft_wraps_and_physical_line_limit() {
        let mut r = rows(&["한글 ", "wrapped", "last"]);
        r[0].wrapped = true;
        assert_eq!(render(&r, 3, true), "한글 wrapped\nlast");
        assert_eq!(render(&r, 2, true), "wrapped\nlast");
    }
}
