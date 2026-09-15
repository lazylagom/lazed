import {
  AppWindowIcon,
  BotIcon,
  ColumnInsertIcon,
  CommandLineIcon,
  FolderImportIcon,
  InformationCircleIcon,
  InsertRowDownIcon,
  ServerIcon,
  WorkflowIcon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import { useCallback, useEffect, useRef, useState } from "react";
import { PaneGrid } from "./components/PaneGrid";
import { About } from "./features/About";
import { AgentPicker } from "./features/AgentPicker";
import { DiffView } from "./features/DiffView";
import { Fanout, type FanoutRequest } from "./features/Fanout";
import { ImportProject } from "./features/ImportProject";
import { PromptBar, type PromptTarget } from "./features/PromptBar";
import { RemoteBar } from "./features/RemoteBar";
import {
  type AgentStatus,
  type HerdrEvent,
  type PaneInfo,
  type Snapshot,
  herdr,
  subscribeEvents,
} from "./shared/herdr";
import { InboxButton, InboxPanel } from "./widgets/Inbox";
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
  "events_reconnect",
]);

function worst(statuses: (AgentStatus | undefined)[]): AgentStatus {
  for (const s of ["blocked", "working", "done", "idle"] as const) {
    if (statuses.includes(s)) return s;
  }
  return "unknown";
}

function remoteLabel(r: { target?: string; session?: string }): string | null {
  if (!r.target) return null;
  return r.session ? `${r.target}·${r.session}` : r.target;
}

async function notify(title: string, body: string) {
  try {
    let granted = await isPermissionGranted();
    if (!granted) granted = (await requestPermission()) === "granted";
    if (granted) sendNotification({ title, body });
  } catch {
    // notifications unavailable — ignore
  }
}

export function App() {
  const [snap, setSnap] = useState<Snapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [warning, setWarning] = useState<string | null>(null);
  const [focusedPane, setFocusedPane] = useState<string | null>(null);
  const [showPicker, setShowPicker] = useState(false);
  const [showPrompt, setShowPrompt] = useState(false);
  const [showFanout, setShowFanout] = useState(false);
  const [diffWsId, setDiffWsId] = useState<string | null>(null);
  const [inboxOpen, setInboxOpen] = useState(false);
  const [dismissed, setDismissed] = useState<ReadonlySet<string>>(new Set());
  const [showRemote, setShowRemote] = useState(false);
  const [showAbout, setShowAbout] = useState(false);
  const [showImport, setShowImport] = useState(false);
  const [remoteTarget, setRemoteTarget] = useState<string | null>(null);
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
    herdr
      .bootstrap()
      .then((res) => {
        if (res.herdr_warning) setWarning(res.herdr_warning);
        refresh();
      })
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
          // a pane that goes back to work re-earns its next inbox entry
          if (status === "working" || status === "idle") {
            setDismissed((prev) => {
              if (!prev.has(paneId)) return prev;
              const next = new Set(prev);
              next.delete(paneId);
              return next;
            });
          }
          if (status === "blocked" || status === "done") {
            const who =
              (d.display_agent as string) ?? (d.agent as string) ?? paneId;
            notify(`${who} is ${status}`, (d.title as string) ?? paneId);
          }
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

  const newWorkspace = useCallback((cwd?: string, label?: string) => {
    setShowImport(false);
    herdr
      .workspaceCreate(cwd, label)
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

  const jumpToPane = useCallback(
    (pane: PaneInfo) => {
      if (pane.workspace_id && pane.workspace_id !== focusedWsId) {
        herdr.workspaceFocus(pane.workspace_id).catch(() => {});
      }
      if (pane.tab_id && pane.tab_id !== activeTabId) {
        herdr.tabFocus(pane.tab_id).catch(() => {});
      }
      setFocusedPane(pane.pane_id);
      setInboxOpen(false);
    },
    [focusedWsId, activeTabId],
  );

  const startAgent = useCallback(
    (kind: string) => {
      const target = effectiveFocusedPane;
      setShowPicker(false);
      if (!target) return;
      herdr.agentStart(target, kind).catch((e) => setError(String(e)));
    },
    [effectiveFocusedPane],
  );

  const submitPrompt = useCallback(
    (text: string, target: PromptTarget) => {
      const wsPanes = [...panesById.values()].filter(
        (p) => p.workspace_id === focusedWsId,
      );
      const agentPanes = wsPanes.filter((p) => p.agent);
      const run = (p: PaneInfo) =>
        (p.agent
          ? herdr.agentPrompt(p.pane_id, text)
          : herdr.paneSendText(p.pane_id, `${text}\n`)
        ).catch((e) => setError(String(e)));
      if (target.kind === "focused") {
        const p = panesById.get(target.paneId);
        if (p) run(p);
      } else if (target.kind === "agents") {
        for (const p of agentPanes) run(p);
      } else {
        for (const p of wsPanes) run(p);
      }
    },
    [panesById, focusedWsId],
  );

  const doFanout = useCallback(async (req: FanoutRequest) => {
    setShowFanout(false);
    if (!req.repo.trim()) {
      setError("fan-out: repo path is required");
      return;
    }
    for (const kind of req.kinds) {
      const branch = `${req.prefix}-${kind}`;
      try {
        const res = await herdr.worktreeCreate(
          req.repo,
          branch,
          req.base,
          branch,
        );
        const paneId = (
          res as { result?: { root_pane?: { pane_id?: string } } }
        )?.result?.root_pane?.pane_id;
        if (!paneId) continue;
        // let the fresh shell pane initialize before launching the agent
        await new Promise((r) => setTimeout(r, 1500));
        await herdr.agentStart(paneId, kind);
        if (req.prompt) {
          // wait until herdr detects the agent (status leaves "unknown"),
          // then a short settle for TUI paint. Bounded: a missed detection
          // degrades to roughly the old fixed delay.
          const deadline = Date.now() + 4000;
          while (Date.now() < deadline) {
            try {
              const a = await herdr.agentGet(paneId);
              if (a.agent_status && a.agent_status !== "unknown") break;
            } catch {
              // agent not registered yet
            }
            await new Promise((r) => setTimeout(r, 250));
          }
          await new Promise((r) => setTimeout(r, 1000));
          await herdr.agentPrompt(paneId, req.prompt);
        }
      } catch (e) {
        setError(String(e));
      }
    }
  }, []);

  const doRemoteConnect = useCallback(
    (target: string, session?: string) => {
      herdr
        .remoteConnect(target, session)
        .then(() => {
          setRemoteTarget(remoteLabel({ target, session }));
          setShowRemote(false);
          setFocusedPane(null);
          refresh();
          // the warning tracks whichever server is now active
          herdr
            .compatWarning()
            .then(setWarning)
            .catch(() => {});
        })
        .catch((e) => {
          setError(String(e));
          setShowRemote(false);
          // a failed connect may still have detached a previous attachment
          herdr
            .remoteStatus()
            .then((r) => setRemoteTarget(remoteLabel(r)))
            .catch(() => {});
          herdr
            .compatWarning()
            .then(setWarning)
            .catch(() => {});
        });
    },
    [refresh],
  );

  const doRemoteDisconnect = useCallback(() => {
    herdr
      .remoteDisconnect()
      .then(() => {
        setRemoteTarget(null);
        setShowRemote(false);
        setFocusedPane(null);
        refresh();
        herdr
          .compatWarning()
          .then(setWarning)
          .catch(() => {});
      })
      .catch((e) => setError(String(e)));
  }, [refresh]);

  useEffect(() => {
    herdr
      .remoteStatus()
      .then((r) => setRemoteTarget(remoteLabel(r)))
      .catch(() => {});
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
        setShowImport(true);
      } else if (e.metaKey && e.shiftKey && e.key === "]") {
        e.preventDefault();
        const next = step(wsIds, focusedWsId, 1);
        if (next) focusWorkspace(next);
      } else if (e.metaKey && e.shiftKey && e.key === "[") {
        e.preventDefault();
        const prev = step(wsIds, focusedWsId, -1);
        if (prev) focusWorkspace(prev);
      } else if (e.metaKey && !e.shiftKey && e.key === "k") {
        e.preventDefault();
        setShowPrompt(true);
      } else if (e.metaKey && e.shiftKey && (e.key === "a" || e.key === "A")) {
        e.preventDefault();
        setShowPicker(true);
      } else if (e.metaKey && e.shiftKey && (e.key === "i" || e.key === "I")) {
        e.preventDefault();
        setInboxOpen((o) => !o);
      } else if (e.metaKey && e.shiftKey && (e.key === "f" || e.key === "F")) {
        e.preventDefault();
        setShowFanout(true);
      } else if (e.metaKey && e.shiftKey && (e.key === "r" || e.key === "R")) {
        e.preventDefault();
        setShowRemote(true);
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
    focusWorkspace,
    focusTab,
  ]);

  const allPanes = [...panesById.values()];
  const inboxItems = allPanes
    .filter(
      (p) =>
        (p.agent_status === "blocked" || p.agent_status === "done") &&
        !dismissed.has(p.pane_id),
    )
    .sort((a, b) =>
      a.agent_status === b.agent_status
        ? 0
        : a.agent_status === "blocked"
          ? -1
          : 1,
    )
    .map((pane) => ({
      pane,
      workspace: snap?.workspaces?.find(
        (w) => w.workspace_id === pane.workspace_id,
      )?.label,
      tab: snap?.tabs?.find((t) => t.tab_id === pane.tab_id)?.label,
    }));
  const blockedCount = inboxItems.filter(
    (i) => i.pane.agent_status === "blocked",
  ).length;
  const agentCount = allPanes.filter((p) => p.agent).length;

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
        <span className="title">STAY LAZY</span>
        <button type="button" onClick={() => split("right")} title="⌘D">
          <HugeiconsIcon icon={ColumnInsertIcon} size={13} strokeWidth={1.5} />
          split →
        </button>
        <button type="button" onClick={() => split("down")} title="⇧⌘D">
          <HugeiconsIcon icon={InsertRowDownIcon} size={13} strokeWidth={1.5} />
          split ↓
        </button>
        <button type="button" onClick={newTab} title="⌘T">
          <HugeiconsIcon icon={AppWindowIcon} size={13} strokeWidth={1.5} />
          tab
        </button>
        <button
          type="button"
          onClick={() => setShowImport(true)}
          title="⇧⌘N — import a project directory as a workspace"
        >
          <HugeiconsIcon icon={FolderImportIcon} size={13} strokeWidth={1.5} />
          import
        </button>
        <button
          type="button"
          onClick={() => setShowPicker(true)}
          title="⇧⌘A — start agent in focused pane"
        >
          <HugeiconsIcon icon={BotIcon} size={13} strokeWidth={1.5} />
          agent
        </button>
        <button
          type="button"
          onClick={() => setShowPrompt(true)}
          title="⌘K — prompt"
        >
          <HugeiconsIcon icon={CommandLineIcon} size={13} strokeWidth={1.5} />
          prompt
        </button>
        <button
          type="button"
          onClick={() => setShowFanout(true)}
          title="⇧⌘F — fan out worktrees × agents"
        >
          <HugeiconsIcon icon={WorkflowIcon} size={13} strokeWidth={1.5} />
          fan-out
        </button>
        <button
          type="button"
          className={remoteTarget ? "remote-on" : ""}
          onClick={() => setShowRemote(true)}
          title="⇧⌘R — attach remote herdr over SSH"
        >
          <HugeiconsIcon icon={ServerIcon} size={13} strokeWidth={1.5} />
          {remoteTarget ? remoteTarget : "remote"}
        </button>
        <InboxButton
          blocked={blockedCount}
          done={inboxItems.length - blockedCount}
          onToggle={() => setInboxOpen((o) => !o)}
        />
        <button
          type="button"
          onClick={() => setShowAbout(true)}
          title="about / licenses"
        >
          <HugeiconsIcon
            icon={InformationCircleIcon}
            size={13}
            strokeWidth={1.5}
          />
        </button>
        <span className="status">
          {error
            ? `error: ${error}`
            : warning
              ? `⚠ ${warning}`
              : snap
                ? `${remoteTarget ? `⇄ ${remoteTarget} · ` : ""}${workspace?.label ?? focusedWsId ?? "?"} · ${layout?.panes.length ?? 0} pane(s)`
                : "connecting…"}
        </span>
      </div>
      {inboxOpen && (
        <InboxPanel
          items={inboxItems}
          onJump={jumpToPane}
          onDismiss={(id) => setDismissed((prev) => new Set(prev).add(id))}
          onDismissAll={() =>
            setDismissed(
              (prev) =>
                new Set([...prev, ...inboxItems.map((i) => i.pane.pane_id)]),
            )
          }
          onClose={() => setInboxOpen(false)}
        />
      )}
      {showPicker && (
        <AgentPicker onPick={startAgent} onClose={() => setShowPicker(false)} />
      )}
      {showPrompt && (
        <PromptBar
          focusedPane={effectiveFocusedPane}
          agentCount={agentCount}
          paneCount={allPanes.length}
          onSubmit={submitPrompt}
          onClose={() => setShowPrompt(false)}
        />
      )}
      {showFanout && (
        <Fanout
          defaultRepo={
            workspace?.worktree?.repo_root ??
            panesById.get(effectiveFocusedPane ?? "")?.cwd ??
            ""
          }
          onSubmit={doFanout}
          onClose={() => setShowFanout(false)}
        />
      )}
      {showAbout && <About onClose={() => setShowAbout(false)} />}
      {showImport && (
        <ImportProject
          onImport={newWorkspace}
          onClose={() => setShowImport(false)}
        />
      )}
      {showRemote && (
        <RemoteBar
          connected={remoteTarget ?? undefined}
          focusedPane={effectiveFocusedPane}
          onConnect={doRemoteConnect}
          onDisconnect={doRemoteDisconnect}
          onClose={() => setShowRemote(false)}
        />
      )}
      {diffWsId &&
        (() => {
          const ws = snap?.workspaces.find((w) => w.workspace_id === diffWsId);
          if (!ws) return null;
          const agentPane = allPanes.find(
            (p) => p.workspace_id === diffWsId && p.agent,
          );
          return (
            <DiffView
              workspace={ws}
              agentPaneId={agentPane?.pane_id}
              onClose={() => setDiffWsId(null)}
              onJump={() => {
                if (agentPane) jumpToPane(agentPane);
              }}
            />
          );
        })()}
      <div className="body">
        <Sidebar
          snap={snap}
          focusedWsId={focusedWsId}
          activeTabId={activeTabId}
          focusedPane={effectiveFocusedPane}
          onFocusWorkspace={focusWorkspace}
          onFocusTab={focusTab}
          onFocusPane={setFocusedPane}
          onNewWorkspace={() => setShowImport(true)}
          onNewTab={newTab}
          onCloseWorkspace={(id) =>
            herdr.workspaceClose(id).catch((e) => setError(String(e)))
          }
          onCloseTab={(id) =>
            herdr.tabClose(id).catch((e) => setError(String(e)))
          }
          onRenameWorkspace={(id, label) =>
            herdr.workspaceRename(id, label).catch((e) => setError(String(e)))
          }
          onRenameTab={(id, label) =>
            herdr.tabRename(id, label).catch((e) => setError(String(e)))
          }
          onDiff={(ws) => setDiffWsId(ws.workspace_id)}
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
