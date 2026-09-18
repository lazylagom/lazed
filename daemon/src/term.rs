//! One terminal = one PTY + one alacritty screen model + scrollback.
use alacritty_terminal::vte::ansi::Processor;
#[path = "terminal_protocol.rs"]
mod protocol;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::Line;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term as AlTerm, TermDamage, TermMode};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde_json::{json, Value};
use std::io::{Read, Write};
use crate::server::ClientSender;
use std::sync::{Arc, Condvar, Mutex};

/// Lock a mutex, recovering from poison — a panic in one thread must not
/// wedge every thread that later touches the same mutex.
pub fn lock<'a, T>(m: &'a Mutex<T>) -> std::sync::MutexGuard<'a, T> {
    m.lock().unwrap_or_else(|e| {
        eprintln!("lazed: recovered poisoned mutex");
        e.into_inner()
    })
}

use crate::render::{row_ansi, write_clear_below, write_cursor, write_row, Cursor, Style};
use std::collections::VecDeque;

/// Bytes of raw PTY output retained per terminal for session restore replay.
const TAIL_CAP: usize = 256 * 1024;
/// Render pacing: dirty state is flushed at most this often — output floods
/// coalesce into one frame instead of streaming byte-for-byte.
const FRAME_MS: u64 = 8;

pub fn validate_size(cols: usize, rows: usize) -> Result<(), String> {
    if !(2..=1000).contains(&cols) || !(1..=500).contains(&rows) || cols * rows > 100_000 {
        return Err("terminal size requires 2..1000 columns, 1..500 rows and at most 100000 cells".into());
    }
    Ok(())
}

#[cfg(test)]
mod size_tests {
    use super::validate_size;

    #[test]
    fn rejects_invalid_sizes_before_allocating() {
        for (cols, rows) in [(0, 24), (1, 24), (80, 0), (65536, 24), (80, usize::MAX), (1000, 500)] {
            assert!(validate_size(cols, rows).is_err(), "{cols}x{rows}");
        }
        for (cols, rows) in [(2, 1), (80, 24), (1000, 100), (200, 500)] {
            assert!(validate_size(cols, rows).is_ok(), "{cols}x{rows}");
        }
    }
}

#[derive(Clone)]
pub struct Sz {
    pub cols: usize,
    pub lines: usize,
}

impl Dimensions for Sz {
    fn total_lines(&self) -> usize {
        self.lines
    }
    fn screen_lines(&self) -> usize {
        self.lines
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// The writer end of one attached client — (id, sender). An explicit id is
/// used because `Sender::same_channel` is still unstable.
pub type Outbox = (u64, ClientSender);

/// Sync primitives shared between the PTY reader, the frame pacer, and
/// anyone locking the term. Kept OUTSIDE the PtyTerm mutex — the render
/// loop must be able to sleep on the condvar without starving term.lock().
pub struct TermSync {
    pub dirty: Mutex<bool>,
    pub cv: Condvar,
    pub dead: std::sync::atomic::AtomicBool,
}

impl TermSync {
    fn is_dead(&self) -> bool {
        self.dead.load(std::sync::atomic::Ordering::Relaxed)
    }
    fn set_dead(&self) {
        self.dead.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

pub struct PtyTerm {
    pub id: String,
    pub cwd: String,
    pub command: String,
    pub label: Option<String>,
    pub kind: String, // "worktree" | "plain"
    /// git branch of the checkout at spawn — the display name for the tab.
    pub branch: Option<String>,
    child: Box<dyn Child + Send + Sync>,
    master: Box<dyn MasterPty + Send>,
    /// Behind its own mutex so PTY writes happen WITHOUT holding the term
    /// lock — a child that stops draining input stalls the writer only.
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    term: AlTerm<protocol::Listener>,
    replies: protocol::Replies,
    proc: Processor,
    size: Sz,
    tail: VecDeque<u8>,
    prev_rows: Vec<String>,
    prev_cursor: Option<Cursor>,
    subs: Vec<Outbox>,
    /// Last emitted (offset, max, cols, rows) — the tuple carries the
    /// viewport size so a resize re-emits term.scroll even when the scroll
    /// offsets themselves didn't move.
    emitted_scroll: (usize, usize, usize, usize),
    pub agent_kind: Option<String>,
    pub agent_status: String,
    pub lifecycle: crate::agent::Lifecycle,
    pub agent_control: Arc<Mutex<()>>,
    pub history_reading: bool,
    pub maintenance: bool,
    pub user_scrolled: bool,
    pub input_seq: u64,
    pub output_seq: u64,
    scroll_px: f64,
    seq: u64,
    sync: Arc<TermSync>,
}

/// The checkout's current branch — symbolic-ref for a named branch,
/// rev-parse fallback for detached HEAD, None outside a repo.
pub fn detect_branch(cwd: &str) -> Option<String> {
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    git(&["symbolic-ref", "--short", "-q", "HEAD"])
        .or_else(|| git(&["rev-parse", "--short", "HEAD"]))
}

fn shell_command(command: &str) -> (String, Vec<String>) {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    if command.is_empty() {
        (shell, vec!["-l".into()])
    } else {
        (shell, vec!["-lc".into(), command.to_string()])
    }
}

impl PtyTerm {
    /// Build the terminal + its PTY reader. The caller wraps the term in
    /// Arc<Mutex<>>, starts `pump(reader)` + `render_loop` threads.
    pub fn spawn(
        id: &str,
        cwd: &str,
        command: &str,
        label: Option<String>,
        kind: &str,
        cols: usize,
        rows: usize,
        workspace_id: &str,
    ) -> anyhow::Result<(PtyTerm, Box<dyn Read + Send>)> {
        validate_size(cols, rows).map_err(anyhow::Error::msg)?;
        let size = Sz { cols, lines: rows };
        let pair = native_pty_system().openpty(PtySize {
            rows: rows as u16,
            cols: cols as u16,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let (prog, args) = shell_command(command);
        let mut cmd = CommandBuilder::new(prog);
        for a in args {
            cmd.arg(a);
        }
        cmd.cwd(cwd);
        protocol::configure_environment(&mut cmd);
        cmd.env("LAZED_TERM", id);
        cmd.env("LAZED_WORKSPACE", workspace_id);
        let child = pair.slave.spawn_command(cmd)?;
        let reader = pair.master.try_clone_reader()?;
        let writer = Arc::new(Mutex::new(pair.master.take_writer()?));
        let (listener, replies) = protocol::connect(writer.clone());

        let mut term_cfg = Config::default();
        term_cfg.scrolling_history = 100_000;
        let term = AlTerm::new(term_cfg, &size, listener);

        let term = PtyTerm {
            id: id.to_string(),
            cwd: cwd.to_string(),
            command: command.to_string(),
            label,
            kind: kind.to_string(),
            branch: detect_branch(cwd),
            child,
            master: pair.master,
            writer,
            term,
            replies,
            proc: Processor::new(),
            size,
            tail: VecDeque::new(),
            prev_rows: Vec::new(),
            prev_cursor: None,
            subs: Vec::new(),
            emitted_scroll: (usize::MAX, usize::MAX, usize::MAX, usize::MAX),
            agent_kind: None,
            agent_status: "unknown".to_string(),
            lifecycle: crate::agent::Lifecycle::default(),
            agent_control: Arc::new(Mutex::new(())),
            history_reading: false,
            maintenance: false,
            user_scrolled: false,
            input_seq: 0,
            output_seq: 0,
            scroll_px: 0.0,
            seq: 0,
            sync: Arc::new(TermSync {
                dirty: Mutex::new(false),
                cv: Condvar::new(),
                dead: std::sync::atomic::AtomicBool::new(false),
            }),
        };
        Ok((term, reader))
    }

    /// Feed raw PTY bytes into the screen model.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.output_seq = self.output_seq.wrapping_add(1);
        for &b in bytes {
            self.proc.advance(&mut self.term, b);
        }
        self.replies.flush(&self.term);
        self.tail.extend(bytes.iter().copied());
        if self.tail.len() > TAIL_CAP {
            let cut = self.tail.len() - TAIL_CAP;
            self.tail.drain(..cut);
        }
        self.mark_dirty();
    }

    /// Blocking PTY read loop — runs on the terminal's reader thread.
    /// Returns when the child dies or the PTY closes.
    pub fn pump(term: &Arc<Mutex<PtyTerm>>, reader: &mut dyn Read) {
        let mut buf = [0u8; 65536];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    lock(term).feed(&buf[..n]);
                }
                Err(_) => break,
            }
        }
        lock(term).set_dead();
    }

    fn mark_dirty(&self) {
        *lock(&self.sync.dirty) = true;
        self.sync.cv.notify_all();
    }

    fn set_dead(&mut self) {
        self.sync.set_dead();
        self.broadcast(json!({"type": "term.closed", "term_id": self.id}));
        self.mark_dirty();
    }

    pub fn is_dead(&self) -> bool {
        self.sync.is_dead()
    }

    /// Frame pacer — runs on the terminal's render thread. Flushes dirty
    /// viewport state at most every FRAME_MS so floods coalesce. The
    /// condvar wait only holds the TermSync lock — never the term lock —
    /// so readers and API calls are never starved.
    pub fn render_loop(term: &Arc<Mutex<PtyTerm>>) {
        let sync = lock(term).sync.clone();
        loop {
            {
                let mut d = lock(&sync.dirty);
                if sync.is_dead() {
                    return;
                }
                if !*d {
                    let (g, _) = sync
                        .cv
                        .wait_timeout(d, std::time::Duration::from_millis(FRAME_MS))
                        .unwrap_or_else(|e| e.into_inner());
                    d = g;
                    if !*d {
                        continue;
                    }
                }
                *d = false;
            }
            if sync.is_dead() {
                return;
            }
            let mut t = lock(term);
            t.emit_frame(false);
            t.emit_scroll();
        }
    }

    /// Serialize one viewport row (respecting scroll offset) to ANSI.
    /// Visible row i maps to buffer Line(i - display_offset); scrolled
    /// views reach into negative (scrollback) lines.
    fn row_string(&self, visible_row: usize) -> String {
        let grid = self.term.grid();
        let row = &grid[Line(visible_row as i32 - grid.display_offset() as i32)];
        row_ansi(row.into_iter().filter_map(|cell| {
            (!cell.flags.contains(Flags::WIDE_CHAR_SPACER)).then_some((
                cell.c,
                Style {
                    fg: cell.fg,
                    bg: cell.bg,
                    flags: cell.flags,
                },
            ))
        }))
    }

    /// Emit an ANSI diff frame to subscribers if the viewport changed.
    ///
    /// alacritty's damage tracking tells us which rows changed since the
    /// previous frame — scroll/resize/mode flips report Full damage — so
    /// only those rows are serialized. Serialized rows are still compared
    /// against `prev_rows` because damage can cover no-op writes (e.g. the
    /// cursor row is always marked damaged).
    pub fn emit_frame(&mut self, force_full: bool) {
        let (damage_full, damaged): (bool, Vec<usize>) = match self.term.damage() {
            TermDamage::Full => (true, Vec::new()),
            TermDamage::Partial(lines) => (false, lines.map(|d| d.line).collect()),
        };
        self.term.reset_damage();
        let full = force_full || damage_full;
        let mut out: Vec<u8> = Vec::with_capacity(self.size.lines * 40);
        let mut hits = damaged.iter().peekable();
        for i in 0..self.size.lines {
            while matches!(hits.peek(), Some(&&l) if l < i) {
                hits.next();
            }
            let hit = matches!(hits.peek(), Some(&&l) if l == i);
            if hit {
                hits.next();
            }
            if !(full || hit) {
                continue;
            }
            let row = self.row_string(i);
            if !full && self.prev_rows.get(i) == Some(&row) {
                continue;
            }
            write_row(&mut out, i, &row);
            if self.prev_rows.len() <= i {
                self.prev_rows.resize(i + 1, String::new());
            }
            self.prev_rows[i] = row;
        }
        // shrink/grow mismatch: clear leftover region
        if self.prev_rows.len() > self.size.lines {
            write_clear_below(&mut out, self.size.lines);
            self.prev_rows.truncate(self.size.lines);
        }
        // Row painting moves the client cursor. Restore the application's
        // cursor after every repaint, and emit cursor-only changes even
        // without new text.
        let cursor = Cursor::from_term(&self.term);
        if !out.is_empty() || full || self.prev_cursor != Some(cursor) {
            write_cursor(&mut out, cursor);
        }
        self.prev_cursor = Some(cursor);
        if out.is_empty() {
            return;
        }
        self.seq += 1;
        let msg = json!({
            "type": "term.frame",
            "term_id": self.id,
            "seq": self.seq,
            "full": full,
            "bytes": base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD, out),
        });
        self.broadcast(msg);
    }

    /// Push scroll metrics when they changed (offset from bottom / max /
    /// viewport size). Size rides along because this is the client's only
    /// resync signal for alt-screen apps — their scroll offsets never move,
    /// so a resize that left offsets untouched would go unreported and the
    /// daemon would keep painting rows the client can't show.
    pub fn emit_scroll(&mut self) {
        let m = self.scroll_metrics();
        if m == self.emitted_scroll {
            return;
        }
        self.emitted_scroll = m;
        self.broadcast(json!({
            "type": "term.scroll",
            "term_id": self.id,
            "offset_from_bottom": m.0,
            "max_offset_from_bottom": m.1,
            "viewport_cols": m.2,
            "viewport_rows": m.3,
        }));
    }

    pub fn scroll_metrics(&self) -> (usize, usize, usize, usize) {
        (
            self.term.grid().display_offset(),
            self.term.grid().history_size(),
            self.size.cols,
            self.size.lines,
        )
    }

    /// Route a scroll: mouse-reporting apps get SGR wheel bytes, alt-screen
    /// apps without mouse get arrows (alternate scroll), everything else
    /// scrolls the display.
    ///
    /// Returns bytes the caller must write to the PTY — the write happens
    /// WITHOUT the term lock held so a stalled child can't wedge rendering.
    pub fn scroll_lines(
        &mut self,
        delta_lines: f64,
        column: u16,
        row: u16,
        _modifiers: u8,
    ) -> Option<Vec<u8>> {
        let mode = self.term.mode();
        let mouse = mode.intersects(TermMode::MOUSE_MODE);
        let alt = mode.contains(TermMode::ALT_SCREEN);
        if mouse {
            let btn = if delta_lines > 0.0 { 65 } else { 64 };
            let n = (delta_lines.abs().ceil().max(1.0) as usize).min(64);
            let mut out = Vec::with_capacity(n * 12);
            for _ in 0..n {
                out.extend_from_slice(
                    format!("\x1b[<{};{};{}M", btn, column + 1, row + 1).as_bytes(),
                );
            }
            Some(out)
        } else if alt && mode.contains(TermMode::ALTERNATE_SCROLL) {
            let key = if delta_lines > 0.0 { "B" } else { "A" };
            let n = (delta_lines.abs().ceil().max(1.0) as usize).min(64);
            Some(format!("\x1bO{key}").repeat(n).into_bytes())
        } else {
            // our delta>0 = scroll down (wheel convention); alacritty's
            // Delta>0 = toward history — negate for the display path
            self.term
                .scroll_display(Scroll::Delta(-(delta_lines as i32)));
            self.mark_dirty();
            None
        }
    }

    /// Accumulate pixel deltas into whole-line scrolls (sub-line remainder
    /// is kept so small gestures still track 1:1). Returns PTY-bound bytes
    /// like `scroll_lines`.
    pub fn scroll_px(
        &mut self,
        delta_px: f64,
        cell_px: f64,
        col: u16,
        row: u16,
        mods: u8,
    ) -> Option<Vec<u8>> {
        if cell_px <= 0.0 {
            return None;
        }
        self.scroll_px += delta_px;
        let lines = (self.scroll_px / cell_px).trunc();
        self.scroll_px -= lines * cell_px;
        if lines != 0.0 {
            self.scroll_lines(lines, col, row, mods)
        } else {
            None
        }
    }

    /// Absolute scroll: jump straight to an offset from the bottom.
    pub fn scroll_to(&mut self, offset_from_bottom: usize) {
        let cur = self.term.grid().display_offset() as i64;
        let delta = offset_from_bottom as i64 - cur;
        if delta != 0 {
            self.term.scroll_display(Scroll::Delta(delta as i32));
            self.mark_dirty();
        }
    }

    /// Handle to the PTY writer — write through this WITHOUT holding the
    /// term lock (writes can block while a child drains its input queue).
    pub fn writer(&self) -> Arc<Mutex<Box<dyn Write + Send>>> {
        self.writer.clone()
    }

    pub fn resize(&mut self, cols: usize, rows: usize) -> Result<(), String> {
        validate_size(cols, rows)?;
        if cols == self.size.cols && rows == self.size.lines {
            return Ok(());
        }
        self.master.resize(PtySize {
            rows: rows as u16,
            cols: cols as u16,
            pixel_width: 0,
            pixel_height: 0,
        }).map_err(|e| format!("pty resize failed: {e}"))?;
        self.size = Sz { cols, lines: rows };
        self.input_seq = self.input_seq.wrapping_add(1);
        self.history_reading = false;
        self.term.resize(Sz { cols, lines: rows });
        self.emit_frame(true);
        // tell subscribers the new viewport size — clients resync on
        // term.scroll, and without this an alt-screen app (scroll offsets
        // pinned at 0) would never learn the size changed
        self.emit_scroll();
        Ok(())
    }

    /// Live screen text (for `terminal.read` + agent detection). This is
    /// always the real screen — a client-side scroll offset is a view
    /// artifact, not terminal content. Trailing blank rows are trimmed so
    /// `max_lines` slices *content*, not empty bottom rows.
    pub fn screen_text(&self, max_lines: usize) -> String {
        let grid = self.term.grid();
        let mut lines: Vec<String> = Vec::new();
        for i in 0..grid.screen_lines() {
            let mut s = String::new();
            for cell in &grid[Line(i as i32)] {
                if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    continue;
                }
                s.push(cell.c);
            }
            lines.push(s);
        }
        while lines.last().is_some_and(|l| l.trim().is_empty()) {
            lines.pop();
        }
        lines
            .iter()
            .skip(lines.len().saturating_sub(max_lines))
            .map(|l| l.trim_end())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn read_text(&self, source: &str, max_lines: usize) -> Result<String, String> {
        if matches!(source, "visible" | "detection") { return Ok(self.screen_text(max_lines)); }
        if !matches!(source, "" | "recent" | "recent-unwrapped") { return Err("unsupported read source".into()); }
        let grid = self.term.grid();
        let first = -(grid.history_size() as i32);
        let last = grid.screen_lines() as i32;
        let start = first.max(last - max_lines as i32);
        let mut text = String::new();
        for i in start..last {
            let mut row = String::new();
            let mut wrapped = false;
            for cell in &grid[Line(i)] {
                if !cell.flags.contains(Flags::WIDE_CHAR_SPACER) { row.push(cell.c); }
                wrapped |= cell.flags.contains(Flags::WRAPLINE);
            }
            if wrapped && source != "recent" { text.push_str(&row); }
            else { text.push_str(row.trim_end()); text.push('\n'); }
        }
        Ok(text.trim_end_matches('\n').to_owned())
    }

    /// Foreground process group leader pid — a cheap tcgetpgrp(3), no
    /// subprocess. The agent watcher reads this for every terminal, then
    /// resolves them all in one batch ps call.
    pub fn fg_pgid(&self) -> Option<i32> {
        self.master.process_group_leader()
    }

    /// Foreground process of this PTY: (comm, full args) if resolvable.
    pub fn fg_process(&self) -> Option<(String, String)> {
        self.fg_pgid().and_then(crate::agent::fg_process)
    }

    pub fn shell_ready(&self) -> bool {
        if self.is_dead()
            || self.master.process_group_leader().map(|p| p as u32) != self.child.process_id()
        {
            return false;
        }
        let Some((comm, _)) = self.fg_process() else { return false };
        let shell = std::path::Path::new(&comm)
            .file_name().and_then(|s| s.to_str()).unwrap_or("");
        if !matches!(shell.trim_start_matches('-'), "sh" | "zsh" | "bash" | "fish") {
            return false;
        }
        let text = self.screen_text(2);
        self.bracketed_paste() || text.lines().last().is_some_and(|l| {
            let l = l.trim_end();
            l.ends_with('$') || l.ends_with('%') || l.ends_with('#') || l.ends_with('❯')
        })
    }

    pub fn bracketed_paste(&self) -> bool {
        self.term.mode().contains(TermMode::BRACKETED_PASTE)
    }

    pub fn alternate_screen(&self) -> bool {
        self.term.mode().contains(TermMode::ALT_SCREEN)
    }

    pub fn history_mouse_supported(&self) -> bool {
        let mode = self.term.mode();
        mode.contains(TermMode::ALT_SCREEN | TermMode::SGR_MOUSE)
            && mode.intersects(TermMode::MOUSE_MODE)
            && self.term.grid().display_offset() == 0
    }

    pub fn history_rows(&self) -> Vec<crate::history::Row> {
        (0..self.rows()).map(|i| {
            let mut text = String::new();
            let mut wrapped = false;
            for cell in &self.term.grid()[Line(i as i32)] {
                if !cell.flags.contains(Flags::WIDE_CHAR_SPACER) { text.push(cell.c); }
                wrapped |= cell.flags.contains(Flags::WRAPLINE);
            }
            crate::history::Row { text: if wrapped { text } else { text.trim_end().to_owned() }, wrapped }
        }).collect()
    }

    /// Re-detect the agent: foreground process → kind, screen → status.
    /// Broadcasts `term.agent` to subscribers on change. Returns changed?
    pub fn detect_agent(&mut self) -> bool {
        let (comm, args) = self.fg_process().unwrap_or_default();
        self.detect_agent_with(&comm, &args)
    }

    /// detect_agent with the foreground process already resolved — the
    /// agent watcher resolves every terminal's pgid in one batch ps call,
    /// so each terminal avoids two ps spawns per tick.
    pub fn detect_agent_with(&mut self, comm: &str, args: &str) -> bool {
        let screen = self.screen_text(10);
        let (kind, mut status) = crate::agent::classify(comm, args, &screen);
        let process_group = self.master.process_group_leader();
        // Old transcript pages can contain old prompts/spinners. They are not
        // current lifecycle evidence. Process replacement still invalidates it.
        if self.history_reading && kind == self.agent_kind
            && process_group == self.lifecycle.process_group {
            return false;
        }
        if self.lifecycle.process_group.is_some()
            && (self.lifecycle.process_group != process_group || kind.is_none())
        {
            self.lifecycle.launch_id = None;
            self.lifecycle.expected_kind = None;
            self.lifecycle.process_group = None;
            self.lifecycle.hook_status = None;
            self.lifecycle.pending_after = None;
        }
        if kind.is_some() && kind == self.lifecycle.expected_kind {
            self.lifecycle.process_group = process_group;
        }
        self.lifecycle.source = if kind.is_some() { "screen" } else { "process" }.into();
        if kind.is_some() && kind == self.lifecycle.expected_kind {
            if let Some(reported) = &self.lifecycle.hook_status {
                if status != "blocked" {
                    status = reported.clone();
                    self.lifecycle.source = "hook".into();
                }
            }
        } else {
            self.lifecycle.hook_status = None;
        }
        // A working agent that goes quiet finished its turn — surface it as
        // "done". Sticky while the same agent stays idle: it's a completion
        // flag the UI can queue, cleared by the next real transition.
        if kind.is_some() && kind == self.agent_kind {
            match (self.agent_status.as_str(), status.as_str()) {
                ("working", "idle") | ("done", "idle") => status = "done".to_string(),
                _ => {}
            }
        }
        if kind == self.agent_kind && status == self.agent_status {
            return false;
        }
        self.agent_kind = kind;
        self.agent_status = status;
        self.lifecycle.record(&self.agent_status);
        if self.lifecycle.pending_after.is_some_and(|seq| self.lifecycle.activity_seq > seq)
            && matches!(self.agent_status.as_str(), "idle" | "done")
        {
            self.lifecycle.pending_after = None;
        }
        self.broadcast(json!({
            "type": "term.agent",
            "term_id": self.id,
            "agent_kind": self.agent_kind,
            "agent_status": self.agent_status,
            "state_seq": self.lifecycle.state_seq,
            "source": self.lifecycle.source,
        }));
        true
    }

    /// Saved raw output tail (session restore persistence).
    pub fn tail_bytes(&mut self) -> &[u8] {
        self.tail.make_contiguous()
    }

    /// Replay saved output tail into the parser — recreates screen +
    /// scrollback on restore without broadcasting frames.
    pub fn replay(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.proc.advance(&mut self.term, b);
        }
        // Historical queries belong to the old process, not the new shell.
        self.replies.discard();
    }

    pub fn subscribe(&mut self, id: u64, out: ClientSender) {
        self.unsubscribe(id);
        self.subs.push((id, out));
        self.prev_rows.clear();
        self.emit_frame(true);
        self.emitted_scroll = (usize::MAX, usize::MAX, usize::MAX, usize::MAX);
        self.emit_scroll();
    }

    pub fn unsubscribe(&mut self, id: u64) {
        self.subs.retain(|(sid, _)| *sid != id);
    }

    fn broadcast(&mut self, msg: Value) {
        self.subs.retain(|(_, s)| s.send(msg.clone()).is_ok());
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }

    pub fn cols(&self) -> usize {
        self.size.cols
    }

    pub fn rows(&self) -> usize {
        self.size.lines
    }
}
