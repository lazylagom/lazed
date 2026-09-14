import { getCurrentWindow } from "@tauri-apps/api/window";
import { useCallback, useEffect, useRef, useState } from "react";
import { PaneGrid } from "./components/PaneGrid";
import {
  type AgentStatus,
  type HerdrEvent,
  type Snapshot,
  herdr,
  subscribeEvents,
} from "./shared/herdr";
import { Sidebar } from "./widgets/Sidebar";

// herdr event envelopes carry the event name in `event` with underscore naming
const REFRESH_EVENTS = new Set([
  "workspace_created",
  "workspace_updated",
  "workspace_renamed",
  "workspace_closed",
  "workspace_focused",
  "tab_created",
  "tab_closed",
  "tab_focused",
  "tab_renamed",
  "tab_moved",
  "pane_created",
  "pane_closed",
  "pane_updated",
  "pane_exited",
  "pane_agent_detected",
  "layout_updated",
  "events.reconnect",
]);

function worst(statuses: (AgentStatus | undefined)[]): AgentStatus {
  for (const s of ["blocked", "working", "done", "idle"] as const) {
    if (statuses.includes(s)) return s;
  }
  return "unknown";
}

export function App() {
  const [snap, setSnap] = useState<Snapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [focusedPane, setFocusedPane] = useState<string | null>(null);
  const refreshTimer = useRef<number | null>(null);

  const refresh = useCallback(() => {
    herdr
      .snapshot()
      .then(setSnap)
      .catch((e) => setError(String(e)));
  }, []);

  const scheduleRefresh = useCallback(() => {
    if (refreshTimer.current !== null) return;
    refreshTimer.current = window.setTimeout(() => {
      refreshTimer.current = null;
      refresh();
    }, 40);
  }, [refresh]);

  useEffect(() => {
    document.title = "staylazy mounted";
    herdr
      .bootstrap()
      .then(() => refresh())
      .catch((e) => setError(String(e)));
    subscribeEvents((ev: HerdrEvent) => {
      // herdr mixes namings: "pane_created" vs "pane.agent_status_changed"
      const name = (ev.event ?? (ev.data?.type as string) ?? "").replace(
        /\./g,
        "_",
      );
      if (!name) return;
      if (name === "pane_agent_status_changed") {
        const d = ev.data ?? {};
        const paneId = d.pane_id as string | undefined;
        const status = d.agent_status as AgentStatus | undefined;
        if (paneId && status) {
          setSnap((prev) =>
            prev
              ? {
                  ...prev,
                  panes: prev.panes.map((p) =>
                    p.pane_id === paneId
                      ? {
                          ...p,
                          agent_status: status,
                          agent: (d.agent as string) ?? p.agent,
                          display_agent:
                            (d.display_agent as string) ?? p.display_agent,
                          title: (d.title as string) ?? p.title,
                        }
                      : p,
                  ),
                }
              : prev,
          );
        }
        return;
      }
      if (REFRESH_EVENTS.has(name)) scheduleRefresh();
    }).catch((e) => setError(String(e)));
  }, [refresh, scheduleRefresh]);

  const focusedWsId =
    snap?.focused_workspace_id ?? snap?.workspaces?.[0]?.workspace_id;
  const workspace = snap?.workspaces?.find(
    (w) => w.workspace_id === focusedWsId,
  );
  const activeTabId = workspace?.active_tab_id ?? snap?.focused_tab_id;
  const layout = snap?.layouts?.find((l) => l.tab_id === activeTabId);
  const agentsByPane = new Map((snap?.agents ?? []).map((a) => [a.pane_id, a]));
  const panesById = new Map(
    (snap?.panes ?? []).map((p) => {
      const a = agentsByPane.get(p.pane_id);
      return [
        p.pane_id,
        a
          ? {
              ...p,
              agent: a.agent,
              display_agent: a.name ?? a.agent,
              agent_status: a.agent_status ?? p.agent_status,
              title: a.title ?? a.terminal_title ?? p.title,
            }
          : p,
      ];
    }),
  );

  const effectiveFocusedPane =
    focusedPane && panesById.has(focusedPane)
      ? focusedPane
      : (layout?.focused_pane_id ?? layout?.panes[0]?.pane_id ?? null);

  const focusWorkspace = useCallback((id: string) => {
    herdr.workspaceFocus(id).catch((e) => setError(String(e)));
  }, []);

  const focusTab = useCallback((id: string) => {
    herdr.tabFocus(id).catch(() => {});
    setFocusedPane(null);
  }, []);

  const split = useCallback(
    (direction: "right" | "down") => {
      const target = effectiveFocusedPane;
      if (!target) return;
      herdr.splitPane(target, direction).catch((e) => setError(String(e)));
    },
    [effectiveFocusedPane],
  );

  const closePane = useCallback((paneId: string) => {
    setFocusedPane((cur) => (cur === paneId ? null : cur));
    herdr.closePane(paneId).catch((e) => setError(String(e)));
  }, []);

  const newTab = useCallback(() => {
    if (!focusedWsId) return;
    herdr
      .tabCreate(focusedWsId)
      .then((res) => {
        const tabId =
          (res as { result?: { tab?: { tab_id?: string } } })?.result?.tab
            ?.tab_id ?? (res as { tab?: { tab_id?: string } })?.tab?.tab_id;
        if (tabId) return herdr.tabFocus(tabId);
      })
      .catch((e) => setError(String(e)));
  }, [focusedWsId]);

  const newWorkspace = useCallback(() => {
    herdr
      .workspaceCreate()
      .then((res) => {
        const wsId =
          (res as { result?: { workspace?: { workspace_id?: string } } })
            ?.result?.workspace?.workspace_id ??
          (res as { workspace?: { workspace_id?: string } })?.workspace
            ?.workspace_id;
        if (wsId) return herdr.workspaceFocus(wsId);
      })
      .catch((e) => setError(String(e)));
  }, []);

  // app-level shortcuts
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey)) return;
      const wsIds = snap?.workspaces.map((w) => w.workspace_id) ?? [];
      const tabIds =
        snap?.tabs
          .filter((t) => t.workspace_id === focusedWsId)
          .map((t) => t.tab_id) ?? [];
      const step = (list: string[], cur: string | undefined, d: number) => {
        const i = Math.max(0, list.indexOf(cur ?? ""));
        return list[(i + d + list.length) % list.length];
      };
      if (e.metaKey && !e.shiftKey && e.key === "d") {
        e.preventDefault();
        split("right");
      } else if (e.metaKey && e.shiftKey && (e.key === "d" || e.key === "D")) {
        e.preventDefault();
        split("down");
      } else if (e.metaKey && !e.shiftKey && e.key === "w") {
        e.preventDefault();
        if (effectiveFocusedPane) closePane(effectiveFocusedPane);
      } else if (e.metaKey && !e.shiftKey && e.key === "t") {
        e.preventDefault();
        newTab();
      } else if (e.metaKey && e.shiftKey && (e.key === "n" || e.key === "N")) {
        e.preventDefault();
        newWorkspace();
      } else if (e.metaKey && e.shiftKey && e.key === "]") {
        e.preventDefault();
        const next = step(wsIds, focusedWsId, 1);
        if (next) focusWorkspace(next);
      } else if (e.metaKey && e.shiftKey && e.key === "[") {
        e.preventDefault();
        const prev = step(wsIds, focusedWsId, -1);
        if (prev) focusWorkspace(prev);
      } else if (e.metaKey && /^[1-9]$/.test(e.key)) {
        e.preventDefault();
        const tab = tabIds[Number(e.key) - 1];
        if (tab) focusTab(tab);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [
    snap,
    focusedWsId,
    effectiveFocusedPane,
    split,
    closePane,
    newTab,
    newWorkspace,
    focusWorkspace,
    focusTab,
  ]);

  return (
    <div className="app">
      <div
        className="titlebar"
        data-tauri-drag-region
        onMouseDown={(e) => {
          if (e.button === 0 && e.target === e.currentTarget) {
            getCurrentWindow()
              .startDragging()
              .catch(() => {});
          }
        }}
      >
        <span className="title">staylazy</span>
        <button type="button" onClick={() => split("right")} title="⌘D">
          split →
        </button>
        <button type="button" onClick={() => split("down")} title="⇧⌘D">
          split ↓
        </button>
        <button type="button" onClick={newTab} title="⌘T">
          + tab
        </button>
        <button type="button" onClick={newWorkspace} title="⇧⌘N">
          + workspace
        </button>
        <span className="status">
          {error
            ? `error: ${error}`
            : snap
              ? `${workspace?.label ?? focusedWsId ?? "?"} · ${layout?.panes.length ?? 0} pane(s)`
              : "connecting…"}
        </span>
      </div>
      <div className="body">
        <Sidebar
          snap={snap}
          focusedWsId={focusedWsId}
          activeTabId={activeTabId}
          focusedPane={effectiveFocusedPane}
          onFocusWorkspace={focusWorkspace}
          onFocusTab={focusTab}
          onFocusPane={setFocusedPane}
          onNewWorkspace={newWorkspace}
          onNewTab={newTab}
          onCloseWorkspace={(id) =>
            herdr.workspaceClose(id).catch((e) => setError(String(e)))
          }
          onCloseTab={(id) =>
            herdr.tabClose(id).catch((e) => setError(String(e)))
          }
          rollup={worst}
        />
        <div className="main">
          {layout && layout.panes.length > 0 ? (
            <PaneGrid
              layout={layout}
              panesById={panesById}
              focusedPane={effectiveFocusedPane}
              onFocusPane={setFocusedPane}
              onClosePane={closePane}
              onResize={(paneId, dir, amt) =>
                herdr.resizePane(paneId, dir, amt).catch(() => {})
              }
            />
          ) : (
            <div className="empty">
              {error
                ? `failed: ${error}`
                : snap
                  ? "no panes in this tab — ⌘D to split"
                  : "connecting to herdr…"}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
