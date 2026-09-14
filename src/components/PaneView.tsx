import { Channel, invoke } from "@tauri-apps/api/core";
import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal } from "@xterm/xterm";
import { useEffect, useRef } from "react";
import "@xterm/xterm/css/xterm.css";

export interface PaneInfo {
  pane_id: string;
  agent_status?: string;
  cwd?: string;
  focused?: boolean;
}

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
  onClose,
}: {
  pane: PaneInfo;
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

    const frames = new Channel<Frame>();
    frames.onmessage = (msg) => {
      if (msg.type === "terminal.frame" && msg.bytes) {
        term.write(b64ToBytes(msg.bytes));
      } else if (msg.type === "log" && msg.text) {
        term.writeln(`\x1b[33m[herdr] ${msg.text}\x1b[0m`);
      } else if (msg.type === "terminal.closed") {
        term.writeln("\r\n\x1b[31m[pane closed]\x1b[0m");
      }
    };

    invoke("attach_pane", {
      paneId: pane.pane_id,
      cols: term.cols,
      rows: term.rows,
      onFrame: frames,
    }).catch((e) => term.writeln(`\x1b[31mattach failed: ${e}\x1b[0m`));

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
      dataSub.dispose();
      ro.disconnect();
      invoke("detach_pane", { paneId: pane.pane_id }).catch(() => {});
      term.dispose();
      termRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pane.pane_id]);

  return (
    <div className="pane-cell">
      <div className="pane-header">
        <span className={`badge ${pane.agent_status ?? "unknown"}`}>
          {pane.agent_status ?? "unknown"}
        </span>
        <span>{pane.pane_id}</span>
        <span>{pane.cwd}</span>
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
