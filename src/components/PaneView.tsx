import { Channel, invoke } from "@tauri-apps/api/core";
import { FitAddon } from "@xterm/addon-fit";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { Terminal } from "@xterm/xterm";
import { useEffect, useRef } from "react";
import "@xterm/xterm/css/xterm.css";
import type { PaneInfo } from "../shared/herdr";
import { TERMINAL_FONT_FAMILY } from "../terminal/config";
import { IMEOverlay } from "../terminal/ime-overlay";

interface Frame {
  type?: string;
  bytes?: string;
  full?: boolean;
  seq?: number;
  text?: string;
}

function b64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

export function PaneView({
  pane,
  focused,
  onFocus,
  onClose,
}: {
  pane: PaneInfo;
  focused: boolean;
  onFocus: () => void;
  onClose: (paneId: string) => void;
}) {
  const hostRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const imeRef = useRef<IMEOverlay | null>(null);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;

    // Font stack mirrors laze — do not add SF Mono: it breaks Hangul
    // composition in the WKWebView (see laze docs/korean-ime.md).
    const term = new Terminal({
      fontFamily: TERMINAL_FONT_FAMILY,
      fontSize: 12,
      fontWeight: 500,
      fontWeightBold: 700,
      lineHeight: 1.2,
      cursorBlink: true,
      cursorStyle: "bar",
      allowProposedApi: true,
      macOptionClickForcesSelection: true,
      theme: { background: "#1e2022" },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.loadAddon(new Unicode11Addon());
    term.unicode.activeVersion = "11";
    term.loadAddon(new WebLinksAddon());
    term.open(host);
    fit.fit();
    termRef.current = term;

    // Korean IME: the overlay input owns all keyboard/IME input and writes
    // to the pane directly; xterm only renders. App-level Cmd+* shortcuts
    // still bubble to the window keydown handler.
    const ime = new IMEOverlay(term, host, (text) => {
      invoke("pane_input", { paneId: pane.pane_id, text }).catch(() => {});
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
        if (msg.type === "terminal.frame" && msg.bytes) {
          retries = 0;
          term.write(b64ToBytes(msg.bytes));
        } else if (msg.type === "log" && msg.text) {
          term.writeln(`\x1b[33m[herdr] ${msg.text}\x1b[0m`);
        } else if (msg.type === "terminal.closed") {
          if (disposed) return;
          // stream ended (server restart or pane gone) — retry attach
          if (retries < 20) {
            retries += 1;
            retryTimer = window.setTimeout(attach, 1000);
          } else {
            term.writeln("\r\n\x1b[31m[pane stream ended]\x1b[0m");
          }
        }
      };
      invoke("attach_pane", {
        paneId: pane.pane_id,
        cols: term.cols,
        rows: term.rows,
        onFrame: frames,
      }).catch((e) => {
        if (disposed) return;
        if (retries < 20) {
          retries += 1;
          retryTimer = window.setTimeout(attach, 1000);
        } else {
          term.writeln(`\x1b[31mattach failed: ${e}\x1b[0m`);
        }
      });
    };
    attach();

    const dataSub = term.onData((text) => {
      invoke("pane_input", { paneId: pane.pane_id, text }).catch(() => {});
    });

    const ro = new ResizeObserver(() => {
      fit.fit();
      invoke("pane_resize", {
        paneId: pane.pane_id,
        cols: term.cols,
        rows: term.rows,
      }).catch(() => {});
    });
    ro.observe(host);

    return () => {
      disposed = true;
      if (retryTimer !== null) window.clearTimeout(retryTimer);
      dataSub.dispose();
      ro.disconnect();
      invoke("detach_pane", { paneId: pane.pane_id }).catch(() => {});
      ime.dispose();
      imeRef.current = null;
      term.dispose();
      termRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pane.pane_id]);

  // When the pane gains focus programmatically (sidebar, inbox, shortcuts),
  // move keyboard focus to the overlay input — unless focus is currently in
  // a UI element outside the terminals (modal input, prompt bar, ...).
  useEffect(() => {
    if (!focused) return;
    const active = document.activeElement;
    if (active instanceof HTMLElement && active !== document.body) {
      if (!active.closest(".pane-term")) return;
    }
    imeRef.current?.focus();
  }, [focused]);

  const label = pane.display_agent ?? pane.agent ?? pane.title ?? pane.pane_id;

  return (
    <div
      className={`pane-cell ${focused ? "focused" : ""}`}
      onMouseDown={onFocus}
      role="presentation"
    >
      <div className="pane-header">
        <span className={`badge ${pane.agent_status ?? "unknown"}`}>
          {pane.agent_status ?? "unknown"}
        </span>
        <span className="pane-label">{label}</span>
        <span className="pane-cwd">{pane.foreground_cwd ?? pane.cwd}</span>
        <button
          type="button"
          className="close"
          onClick={() => onClose(pane.pane_id)}
          title="close pane"
        >
          ✕
        </button>
      </div>
      <div ref={hostRef} className="pane-term" />
    </div>
  );
}
