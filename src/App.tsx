import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import { PaneView, type PaneInfo } from "./components/PaneView";

interface Bootstrap {
  workspace: { workspace_id?: string; label?: string };
  panes: PaneInfo[];
}

export function App() {
  const [boot, setBoot] = useState<Bootstrap | null>(null);
  const [panes, setPanes] = useState<PaneInfo[]>([]);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    const res = await invoke<{ panes: PaneInfo[] }>("list_panes");
    setPanes(res.panes);
  }, []);

  useEffect(() => {
    invoke<Bootstrap>("bootstrap")
      .then((b) => {
        setBoot(b);
        setPanes(b.panes);
      })
      .catch((e) => setError(String(e)));
  }, []);

  const split = async (direction: "right" | "down") => {
    const target = panes[panes.length - 1];
    if (!target) return;
    await invoke("split_pane", { paneId: target.pane_id, direction }).catch(
      (e) => setError(String(e)),
    );
    await refresh();
  };

  const close = async (paneId: string) => {
    await invoke("close_pane", { paneId }).catch((e) => setError(String(e)));
    await refresh();
  };

  return (
    <div className="app">
      <div className="titlebar">
        <span className="title">staylazy</span>
        <button type="button" onClick={() => split("right")}>
          split →
        </button>
        <button type="button" onClick={() => split("down")}>
          split ↓
        </button>
        <span className="status">
          {error
            ? `error: ${error}`
            : boot
              ? `workspace ${boot.workspace.workspace_id ?? "?"} · ${panes.length} pane(s)`
              : "connecting…"}
        </span>
      </div>
      <div className="panes">
        {panes.length === 0 ? (
          <div className="empty">
            {error ? `failed: ${error}` : "no panes — attach a workspace first"}
          </div>
        ) : (
          panes.map((p) => <PaneView key={p.pane_id} pane={p} onClose={close} />)
        )}
      </div>
    </div>
  );
}
