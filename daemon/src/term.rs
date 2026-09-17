//! One terminal = one PTY + one alacritty screen model + scrollback.
use alacritty_terminal::vte::ansi::Processor;
#[path = "terminal_protocol.rs"]
mod protocol;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::Line;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term as AlTerm, TermMode};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};

/// Lock a mutex, recovering from poison — a panic in one thread must not
/// wedge every thread that later touches the same mutex.
pub fn lock<'a, T>(m: &'a Mutex<T>) -> std::sync::MutexGuard<'a, T> {
    m.lock().unwrap_or_else(|e| {
        eprintln!("lazed: recovered poisoned mutex");
        e.into_inner()
    })
}

use crate::render::{diff_frame, row_ansi, Cursor, Style};

/// Bytes of raw PTY output retained per terminal for session restore replay.
const TAIL_CAP: usize = 256 * 1024;
/// Render pacing: dirty state is flushed at most this often — output floods
/// coalesce into one frame instead of streaming byte-for-byte.
const FRAME_MS: u64 = 8;

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
pub type Outbox = (u64, Sender<Value>);

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
    tail: Vec<u8>,
    prev_rows: Vec<String>,
    prev_cursor: Option<Cursor>,
    subs: Vec<Outbox>,
    emitted_scroll: (usize, usize),
    pub agent_kind: Option<String>,
    pub agent_status: String,
    scroll_px: f64,
    seq: u64,
    sync: Arc<TermSync>,
}

/// The checkout's current branch — symbolic-ref for a named branch,
/// rev-parse fallback for detached HEAD, None outside a repo.
fn detect_branch(cwd: &str) -> Option<String> {
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
    ) -> anyhow::Result<(PtyTerm, Box<dyn Read + Send>)> {
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
            tail: Vec::new(),
            prev_rows: Vec::new(),
            prev_cursor: None,
            subs: Vec::new(),
            emitted_scroll: (usize::MAX, usize::MAX),
            agent_kind: None,
            agent_status: "unknown".to_string(),
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
        for &b in bytes {
            self.proc.advance(&mut self.term, b);
        }
        self.replies.flush(&self.term);
        self.tail.extend_from_slice(bytes);
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

    /// Serialize the current viewport (respecting scroll offset) into
    /// per-row ANSI strings.
    fn viewport_rows(&self) -> Vec<String> {
        let grid = self.term.grid();
        // display_iter yields BUFFER line indices — scrollback lines are
        // negative (Line(-offset) is the topmost visible row) — so the
        // visible row is buffer line + display_offset.
        let offset = grid.display_offset() as i32;
        let mut rows: Vec<Vec<(char, Style)>> = vec![Vec::new(); self.size.lines];
        for indexed in grid.display_iter() {
            let line = indexed.point.line.0 + offset;
            if line < 0 || line as usize >= self.size.lines {
                continue;
            }
            let line = line as usize;
            let cell = indexed.cell;
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            rows[line].push((
                cell.c,
                Style {
                    fg: cell.fg,
                    bg: cell.bg,
                    flags: cell.flags,
                },
            ));
        }
        rows.into_iter()
            .map(|cells| row_ansi(cells.into_iter()))
            .collect()
    }

    /// Emit an ANSI diff frame to subscribers if the viewport changed.
    pub fn emit_frame(&mut self, force_full: bool) {
        let next = self.viewport_rows();
        let cursor = Cursor::from_term(&self.term);
        let bytes = diff_frame(&mut self.prev_rows, next, force_full, &mut self.prev_cursor, cursor);
        if bytes.is_empty() {
            return;
        }
        self.seq += 1;
        let msg = json!({
            "type": "term.frame",
            "term_id": self.id,
            "seq": self.seq,
            "full": force_full,
            "bytes": base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD, bytes),
        });
        self.broadcast(msg);
    }

    /// Push scroll metrics when they changed (offset from bottom / max).
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
            "viewport_rows": self.size.lines,
        }));
    }

    pub fn scroll_metrics(&self) -> (usize, usize) {
        (
            self.term.grid().display_offset(),
            self.term.grid().history_size(),
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

    pub fn resize(&mut self, cols: usize, rows: usize) {
        if cols == 0 || rows == 0 || (cols == self.size.cols && rows == self.size.lines) {
            return;
        }
        self.size = Sz { cols, lines: rows };
        let _ = self.master.resize(PtySize {
            rows: rows as u16,
            cols: cols as u16,
            pixel_width: 0,
            pixel_height: 0,
        });
        self.term.resize(Sz { cols, lines: rows });
        self.emit_frame(true);
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

    /// Foreground process of this PTY: (comm, full args) if resolvable.
    pub fn fg_process(&self) -> Option<(String, String)> {
        self.master
            .process_group_leader()
            .and_then(crate::agent::fg_process)
    }

    /// Re-detect the agent: foreground process → kind, screen → status.
    /// Broadcasts `term.agent` to subscribers on change. Returns changed?
    pub fn detect_agent(&mut self) -> bool {
        let screen = self.screen_text(10);
        let (comm, args) = self.fg_process().unwrap_or_default();
        let (kind, mut status) = crate::agent::classify(&comm, &args, &screen);
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
        self.broadcast(json!({
            "type": "term.agent",
            "term_id": self.id,
            "agent_kind": self.agent_kind,
            "agent_status": self.agent_status,
        }));
        true
    }

    /// Saved raw output tail (session restore persistence).
    pub fn tail_bytes(&self) -> &[u8] {
        &self.tail
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

    pub fn subscribe(&mut self, id: u64, out: Sender<Value>) {
        self.subs.push((id, out));
        self.prev_rows.clear();
        self.emit_frame(true);
        self.emitted_scroll = (usize::MAX, usize::MAX);
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
