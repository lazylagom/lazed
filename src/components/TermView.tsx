import { Channel, invoke } from "@tauri-apps/api/core";
import { FitAddon } from "@xterm/addon-fit";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { Terminal } from "@xterm/xterm";
import { memo, useCallback, useEffect, useRef, useState } from "react";
import "@xterm/xterm/css/xterm.css";
import { openUrl } from "../shared/inbox";
import type { PaneInfo } from "../shared/lazed";
import { TERMINAL_FONT_FAMILY, TERMINAL_THEME } from "../terminal/config";
import { IMEOverlay } from "../terminal/ime-overlay";

/** One NDJSON line from `herdr terminal session control`. */
interface Frame {
  type?: string;
  /** base64 ANSI — a full repaint when `full`, else a viewport diff */
  bytes?: string;
  full?: boolean;
  seq?: number;
  width?: number;
  height?: number;
  text?: string;
}

/** `herdr.pane.scroll_changed` relayed by App as a window event. */
interface PaneScroll {
  pane_id?: string;
  offset_from_bottom?: number;
  max_offset_from_bottom?: number;
  viewport_rows?: number;
}

// Uint8Array.fromBase64 (Baseline 2025 / WKWebView 18.4+) decodes straight
// into bytes; atob + charCodeAt loop is the fallback for older webviews.
const NATIVE_B64: ((s: string) => Uint8Array) | undefined = (
  Uint8Array as unknown as { fromBase64?: (s: string) => Uint8Array }
).fromBase64?.bind(Uint8Array);

function b64ToBytes(b64: string): Uint8Array {
  if (NATIVE_B64) return NATIVE_B64(b64);
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

export const TermView = memo(function TermView({
  term,
  focused,
  onFocus,
  onClose,
}: {
  term: PaneInfo;
  focused: boolean;
  onFocus: (paneId: string) => void;
  onClose: (paneId: string) => void;
}) {
  const hostRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const imeRef = useRef<IMEOverlay | null>(null);
  // herdr-side scroll state: offset = lines above the live edge. Kept in
  // a ref + imperative DOM — scroll events can arrive at frame rate during
  // floods and must not re-render React per message.
  const scrollRef = useRef({
    offset: term.scroll?.offset_from_bottom ?? 0,
    max: term.scroll?.max_offset_from_bottom ?? 0,
    rows: term.scroll?.viewport_rows ?? 1,
  });
  const [scrollable, setScrollable] = useState(false);
  const barRef = useRef<HTMLDivElement>(null);
  const thumbRef = useRef<HTMLDivElement>(null);

  const applyScroll = useCallback(() => {
    const { offset, max, rows } = scrollRef.current;
    const can = max > 0;
    setScrollable((prev) => (prev === can ? prev : can));
    const bar = barRef.current;
    if (bar) bar.classList.toggle("scrolled", offset > 0);
    const thumb = thumbRef.current;
    if (thumb && can) {
      const frac = Math.max(0.04, rows / (rows + max));
      thumb.style.height = `${frac * 100}%`;
      thumb.style.top = `${((max - offset) / (rows + max)) * 100}%`;
    }
  }, []);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;

    // Font stack mirrors laze — do not add SF Mono: it breaks Hangul
    // composition in the WKWebView (see laze docs/korean-ime.md).
    const xterm = new Terminal({
      fontFamily: TERMINAL_FONT_FAMILY,
      fontSize: 12,
      // scrollback lives in the daemon — a local buffer only lets the
      // scrollbar appear (and drag desyncs the mirrored viewport)
      scrollback: 0,
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
    xterm.loadAddon(
      // window.open() is a no-op inside the Tauri webview — route link
      // activation through the `open_url` command (shells out to `open`)
      new WebLinksAddon((_event, uri) => {
        openUrl(uri).catch(() => {});
      }),
    );
    xterm.open(host);
    fit.fit();
    termRef.current = xterm;

    // Korean IME: the overlay input owns all keyboard/IME input and writes
    // to the terminal directly; xterm only renders. App-level Cmd+*
    // shortcuts still bubble to the window keydown handler.
    const ime = new IMEOverlay(xterm, host, (text) => {
      invoke("pane_input", { paneId: term.pane_id, text }).catch(() => {});
    });
    imeRef.current = ime;

    // Push our fitted size back to the daemon when its viewport disagrees —
    // a daemon painting rows/cols we can't show hides the app's input area.
    // Throttled so two disagreeing attached clients can't livelock each
    // other (each resize echoes a fresh term.scroll).
    let lastSizePush = 0;
    let sizePushTimer: number | null = null;
    const pushSize = () => {
      if (disposed) return;
      const now = Date.now();
      const wait = 250 - (now - lastSizePush);
      if (wait > 0) {
        if (sizePushTimer === null) {
          sizePushTimer = window.setTimeout(() => {
            sizePushTimer = null;
            pushSize();
          }, wait);
        }
        return;
      }
      lastSizePush = now;
      invoke("pane_resize", {
        paneId: term.pane_id,
        cols: xterm.cols,
        rows: xterm.rows,
      }).catch(() => {});
    };

    let disposed = false;
    let retries = 0;
    let retryTimer: number | null = null;
    let frames: Channel<Frame> | null = null;

    const attach = () => {
      if (disposed) return;
      frames = new Channel<Frame>();
      frames.onmessage = (msg) => {
        if (msg.type === "terminal.frame" && msg.bytes) {
          retries = 0;
          xterm.write(b64ToBytes(msg.bytes));
        } else if (msg.type === "terminal.closed") {
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
      invoke("pane_attach", {
        paneId: term.pane_id,
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
      invoke("pane_input", { paneId: term.pane_id, text }).catch(() => {});
    });

    // Scrollback lives in herdr, not xterm's buffer — the stream only
    // mirrors the visible viewport as ANSI diffs, so xterm's own wheel
    // scroll has nothing to scroll. herdr takes whole-line scroll commands
    // (`terminal.scroll {direction, lines}`), so wheel pixels are turned
    // into lines here, carrying the sub-line remainder across events.
    //
    // Events are batched once per animation frame — trackpads emit ~120/s,
    // so per-event commands would just queue up and trail the gesture.
    let pendingWheelPx = 0;
    let pendingLines = 0;
    let scrollRaf: number | null = null;
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
    // wheel deltaY > 0 = toward the live edge; herdr lines > 0 = toward
    // history, so the sign flips here
    const sendScroll = (lines: number) => {
      if (lines === 0) return;
      invoke("pane_scroll", { paneId: term.pane_id, lines: -lines }).catch(
        () => {},
      );
    };
    const flushScroll = () => {
      scrollRaf = null;
      if (pendingLines !== 0) {
        // line/page delta modes arrive as whole lines already
        const lines = Math.trunc(pendingLines);
        pendingLines -= lines;
        sendScroll(lines);
      }
      if (pendingWheelPx !== 0) {
        const lines = Math.trunc(pendingWheelPx / Math.max(1, wheelCellH));
        pendingWheelPx -= lines * wheelCellH;
        sendScroll(lines);
      }
    };
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      e.stopPropagation();
      const rect = wheelRect();
      const cellH = rect.height / xterm.rows;
      if (!(cellH > 0)) return;
      wheelCellH = cellH;

      if (e.deltaMode === 1) {
        pendingLines += e.deltaY;
      } else if (e.deltaMode === 2) {
        pendingLines += e.deltaY * xterm.rows;
      } else {
        pendingWheelPx += e.deltaY;
      }
      if (scrollRaf === null) {
        scrollRaf = requestAnimationFrame(flushScroll);
      }
    };
    host.addEventListener("wheel", onWheel, { capture: true, passive: false });

    // scroll position comes from herdr (`pane.scroll_changed`), relayed by
    // App as a window event keyed by pane id
    const onPaneScroll = (e: Event) => {
      const d = (e as CustomEvent<PaneScroll>).detail;
      if (!d || d.pane_id !== term.pane_id) return;
      scrollRef.current = {
        offset: d.offset_from_bottom ?? 0,
        max: d.max_offset_from_bottom ?? 0,
        rows: d.viewport_rows ?? xterm.rows,
      };
      applyScroll();
    };
    window.addEventListener("lazed:pane-scroll", onPaneScroll);

    // fit() mutates xterm's DOM, which would queue another RO notification
    // inside the same delivery pass ("ResizeObserver loop completed with
    // undelivered notifications"). Defer to rAF so layout changes land in
    // the next frame, and skip callbacks where the size didn't move.
    let resizeRaf: number | null = null;
    let lastW = 0;
    let lastH = 0;
    const ro = new ResizeObserver((entries) => {
      screenRect = null;
      const { width, height } = entries[0].contentRect;
      if (width === lastW && height === lastH) return;
      lastW = width;
      lastH = height;
      if (resizeRaf === null) {
        resizeRaf = requestAnimationFrame(() => {
          resizeRaf = null;
          if (disposed) return;
          fit.fit();
          invoke("pane_resize", {
            paneId: term.pane_id,
            cols: xterm.cols,
            rows: xterm.rows,
          }).catch(() => {});
        });
      }
    });
    ro.observe(host);

    return () => {
      disposed = true;
      if (retryTimer !== null) window.clearTimeout(retryTimer);
      if (sizePushTimer !== null) window.clearTimeout(sizePushTimer);
      if (scrollRaf !== null) cancelAnimationFrame(scrollRaf);
      if (resizeRaf !== null) cancelAnimationFrame(resizeRaf);
      host.removeEventListener("wheel", onWheel, { capture: true });
      window.removeEventListener("lazed:pane-scroll", onPaneScroll);
      dataSub.dispose();
      ro.disconnect();
      invoke("pane_detach", { paneId: term.pane_id }).catch(() => {});
      ime.dispose();
      imeRef.current = null;
      xterm.dispose();
      termRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [term.pane_id, applyScroll]);

  // The scrollbar nodes mount after scrollable changes; paint them after render.
  useEffect(() => {
    applyScroll();
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

  const label =
    term.label ??
    term.agent_name ??
    term.title ??
    term.display_agent ??
    term.agent ??
    term.pane_id;

  // Thin overlay scrollbar driven by herdr's scroll metrics — display + jump
  // only; a jump is sent as the line delta from the current offset.
  const jumpScroll = (e: React.PointerEvent<HTMLDivElement>) => {
    const { offset, max, rows } = scrollRef.current;
    const thumbFrac = Math.max(0.04, rows / (rows + max));
    const rect = e.currentTarget.getBoundingClientRect();
    const f = (e.clientY - rect.top) / Math.max(1, rect.height) - thumbFrac / 2;
    const t = Math.max(0, Math.min(1 - thumbFrac, f));
    const target = Math.round(max * (1 - t / (1 - thumbFrac)));
    const lines = target - offset;
    if (lines === 0) return;
    invoke("pane_scroll", { paneId: term.pane_id, lines }).catch(() => {});
  };
  const onScrollBarDown = (e: React.PointerEvent<HTMLDivElement>) => {
    e.currentTarget.setPointerCapture(e.pointerId);
    jumpScroll(e);
  };
  const onScrollBarMove = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.buttons & 1) jumpScroll(e);
  };

  return (
    <div
      className={`pane-cell ${focused ? "focused" : ""}`}
      onMouseDown={() => onFocus(term.pane_id)}
      role="presentation"
    >
      <div className="pane-header">
        <span className={`badge ${term.agent_status ?? "unknown"}`}>
          {term.agent_status ?? "unknown"}
        </span>
        <span className="pane-label">{label}</span>
        <span className="pane-cwd">{term.cwd}</span>
        <button
          type="button"
          className="close"
          onClick={() => onClose(term.pane_id)}
          title="close terminal"
        >
          ✕
        </button>
      </div>
      <div
        ref={hostRef}
        className="pane-term"
        onFocusCapture={() => onFocus(term.pane_id)}
      >
        {scrollable && (
          <div
            ref={barRef}
            className="term-scrollbar"
            onPointerDown={onScrollBarDown}
            onPointerMove={onScrollBarMove}
            role="presentation"
          >
            <div ref={thumbRef} className="term-scroll-thumb" />
          </div>
        )}
      </div>
    </div>
  );
});
