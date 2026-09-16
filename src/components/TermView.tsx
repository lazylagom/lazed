import { Channel, invoke } from "@tauri-apps/api/core";
import { FitAddon } from "@xterm/addon-fit";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { Terminal } from "@xterm/xterm";
import { useEffect, useRef } from "react";
import "@xterm/xterm/css/xterm.css";
import type { TerminalInfo } from "../shared/lazed";
import { TERMINAL_FONT_FAMILY, TERMINAL_THEME } from "../terminal/config";
import { IMEOverlay } from "../terminal/ime-overlay";

interface Frame {
  type?: string;
  term_id?: string;
  bytes?: string;
  full?: boolean;
  seq?: number;
  text?: string;
  offset_from_bottom?: number;
  max_offset_from_bottom?: number;
  agent_kind?: string;
  agent_status?: string;
}

function b64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

export function TermView({
  term,
  focused,
  onFocus,
  onClose,
}: {
  term: TerminalInfo;
  focused: boolean;
  onFocus: () => void;
  onClose: (termId: string) => void;
}) {
  const hostRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const imeRef = useRef<IMEOverlay | null>(null);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;

    // Font stack mirrors laze — do not add SF Mono: it breaks Hangul
    // composition in the WKWebView (see laze docs/korean-ime.md).
    const xterm = new Terminal({
      fontFamily: TERMINAL_FONT_FAMILY,
      fontSize: 12,
      fontWeight: 300,
      fontWeightBold: 700,
      lineHeight: 1.2,
      cursorBlink: true,
      cursorStyle: "bar",
      allowProposedApi: true,
      macOptionClickForcesSelection: true,
      theme: TERMINAL_THEME,
    });
    const fit = new FitAddon();
    xterm.loadAddon(fit);
    xterm.loadAddon(new Unicode11Addon());
    xterm.unicode.activeVersion = "11";
    xterm.loadAddon(new WebLinksAddon());
    xterm.open(host);
    fit.fit();
    termRef.current = xterm;

    // Korean IME: the overlay input owns all keyboard/IME input and writes
    // to the terminal directly; xterm only renders. App-level Cmd+*
    // shortcuts still bubble to the window keydown handler.
    const ime = new IMEOverlay(xterm, host, (text) => {
      invoke("term_input", { termId: term.term_id, text }).catch(() => {});
    });
    imeRef.current = ime;

    let disposed = false;
    let retries = 0;
    let retryTimer: number | null = null;
    let frames: Channel<Frame> | null = null;

    const attach = () => {
      if (disposed) return;
      frames = new Channel<Frame>();
      frames.onmessage = (msg) => {
        if (msg.type === "term.frame" && msg.bytes) {
          retries = 0;
          xterm.write(b64ToBytes(msg.bytes));
        } else if (msg.type === "term.scroll") {
          // scroll metrics — available for a future scrollbar indicator
        } else if (msg.type === "term.agent") {
          // per-terminal agent updates also arrive via global events
        } else if (msg.type === "log" && msg.text) {
          xterm.writeln(`\x1b[33m[lazed] ${msg.text}\x1b[0m`);
        } else if (msg.type === "term.closed") {
          if (disposed) return;
          // stream ended (daemon restart or terminal gone) — retry attach
          if (retries < 20) {
            retries += 1;
            retryTimer = window.setTimeout(attach, 1000);
          } else {
            xterm.writeln("\r\n\x1b[31m[terminal stream ended]\x1b[0m");
          }
        }
      };
      invoke("term_attach", {
        termId: term.term_id,
        cols: xterm.cols,
        rows: xterm.rows,
        onFrame: frames,
      }).catch((e) => {
        if (disposed) return;
        if (retries < 20) {
          retries += 1;
          retryTimer = window.setTimeout(attach, 1000);
        } else {
          xterm.writeln(`\x1b[31mattach failed: ${e}\x1b[0m`);
        }
      });
    };
    attach();

    const dataSub = xterm.onData((text) => {
      invoke("term_input", { termId: term.term_id, text }).catch(() => {});
    });

    // Scrollback lives in the daemon, not xterm's buffer — the attach stream
    // only mirrors the visible viewport as ANSI diffs, so xterm's own wheel
    // scroll has nothing to scroll. We forward PIXEL deltas and let the
    // daemon accumulate sub-line remainders + route them (display scroll
    // for shells, SGR wheel bytes for mouse-reporting apps, arrows for
    // alternate-scroll apps).
    //
    // Events are still batched once per animation frame — trackpads emit
    // ~120/s while the daemon paces frames at ~125fps, so per-event commands
    // would just queue up and trail behind the gesture.
    let pendingWheelPx = 0;
    let pendingLines = 0;
    let scrollRaf: number | null = null;
    let wheelPos = { column: 1, row: 1, modifiers: 0 };
    let wheelCellH = 1;
    let screenRect: DOMRect | null = null;
    let screenRectAt = 0;
    const wheelRect = () => {
      // getBoundingClientRect forces layout while xterm repaints — cache it
      // for the length of a gesture instead of reading it per event
      const now = performance.now();
      if (!screenRect || now - screenRectAt > 250) {
        const screen = host.querySelector<HTMLElement>(".xterm-screen") ?? host;
        screenRect = screen.getBoundingClientRect();
        screenRectAt = now;
      }
      return screenRect;
    };
    const flushScroll = () => {
      scrollRaf = null;
      if (pendingLines !== 0) {
        // line/page delta modes arrive as whole lines already
        const lines = pendingLines;
        pendingLines = 0;
        invoke("term_scroll", {
          termId: term.term_id,
          deltaLines: lines,
          ...wheelPos,
        }).catch(() => {});
        return;
      }
      if (pendingWheelPx !== 0) {
        const px = pendingWheelPx;
        pendingWheelPx = 0;
        invoke("term_scroll", {
          termId: term.term_id,
          deltaPx: px,
          cellPx: wheelCellH,
          ...wheelPos,
        }).catch(() => {});
      }
    };
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      e.stopPropagation();
      const rect = wheelRect();
      const cellH = rect.height / xterm.rows;
      const cellW = rect.width / xterm.cols;
      if (!(cellH > 0) || !(cellW > 0)) return;
      wheelCellH = cellH;

      if (e.deltaMode === 1) {
        pendingLines += e.deltaY;
      } else if (e.deltaMode === 2) {
        pendingLines += e.deltaY * xterm.rows;
      } else {
        pendingWheelPx += e.deltaY;
      }

      const clamp = (v: number, max: number) =>
        Math.max(0, Math.min(max - 1, v));
      wheelPos = {
        column: clamp(Math.floor((e.clientX - rect.left) / cellW), xterm.cols),
        row: clamp(Math.floor((e.clientY - rect.top) / cellH), xterm.rows),
        modifiers:
          (e.shiftKey ? 1 : 0) |
          (e.ctrlKey ? 2 : 0) |
          (e.altKey ? 4 : 0) |
          (e.metaKey ? 8 : 0),
      };
      if (scrollRaf === null) {
        scrollRaf = requestAnimationFrame(flushScroll);
      }
    };
    host.addEventListener("wheel", onWheel, { capture: true, passive: false });

    const ro = new ResizeObserver(() => {
      screenRect = null;
      fit.fit();
      invoke("term_resize", {
        termId: term.term_id,
        cols: xterm.cols,
        rows: xterm.rows,
      }).catch(() => {});
    });
    ro.observe(host);

    return () => {
      disposed = true;
      if (retryTimer !== null) window.clearTimeout(retryTimer);
      if (scrollRaf !== null) cancelAnimationFrame(scrollRaf);
      host.removeEventListener("wheel", onWheel, { capture: true });
      dataSub.dispose();
      ro.disconnect();
      invoke("term_detach", { termId: term.term_id }).catch(() => {});
      ime.dispose();
      imeRef.current = null;
      xterm.dispose();
      termRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [term.term_id]);

  // Re-apply the theme on render so live terminals pick up palette changes
  // (xterm only reads `theme` at construction; assigning options.theme repaints).
  useEffect(() => {
    const xterm = termRef.current;
    if (xterm) xterm.options.theme = TERMINAL_THEME;
  });

  // When the terminal gains focus programmatically (sidebar, inbox,
  // shortcuts), move keyboard focus to the overlay input — unless focus is
  // currently in a UI element outside the terminals.
  useEffect(() => {
    if (!focused) return;
    const active = document.activeElement;
    if (active instanceof HTMLElement && active !== document.body) {
      if (!active.closest(".pane-term")) return;
    }
    imeRef.current?.focus();
  }, [focused]);

  const label = term.label ?? term.branch ?? term.agent_kind ?? term.term_id;

  return (
    <div
      className={`pane-cell ${focused ? "focused" : ""}`}
      onMouseDown={onFocus}
      role="presentation"
    >
      <div className="pane-header">
        <span className={`badge ${term.agent_status ?? "unknown"}`}>
          {term.agent_status ?? "unknown"}
        </span>
        <span className="pane-label">
          {term.kind === "worktree" ? "⑂ " : ""}
          {label}
        </span>
        <span className="pane-cwd">{term.cwd}</span>
        <button
          type="button"
          className="close"
          onClick={() => onClose(term.term_id)}
          title="close terminal"
        >
          ✕
        </button>
      </div>
      <div ref={hostRef} className="pane-term" />
    </div>
  );
}
