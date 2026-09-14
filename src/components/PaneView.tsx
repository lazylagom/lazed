import { Channel, invoke } from "@tauri-apps/api/core";
import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal } from "@xterm/xterm";
import { useEffect, useRef } from "react";
import "@xterm/xterm/css/xterm.css";
import type { PaneInfo } from "../shared/herdr";

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

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;

    const term = new Terminal({
      fontFamily: "Menlo, monospace",
      fontSize: 13,
      cursorBlink: true,
      allowProposedApi: true,
      theme: { background: "#1e2022" },
    });
    term.attachCustomKeyEventHandler((e) => {
      // let app-level shortcuts (Cmd+*) bubble to the window handler
      if (e.metaKey) return false;
      return true;
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.loadAddon(new WebLinksAddon());
    term.open(host);
    try {
      term.loadAddon(new WebglAddon());
    } catch {
      // fall back to canvas renderer
    }
    fit.fit();
    termRef.current = term;

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
      term.dispose();
      termRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pane.pane_id]);

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
