import { ArrowShrinkIcon } from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  isPermissionGranted,
  requestPermission,
} from "@tauri-apps/plugin-notification";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { TermGrid } from "./components/TermGrid";
import { AgentPicker } from "./features/AgentPicker";
import { AutomationEditor } from "./features/AutomationEditor";
import { Automations } from "./features/Automations";
import { DiffView } from "./features/DiffView";
import { Fanout, type FanoutRequest } from "./features/Fanout";
import { ImportProject } from "./features/ImportProject";
import { NewWorktree, type NewWorktreeRequest } from "./features/NewWorktree";
import { PromptBar, type PromptTarget } from "./features/PromptBar";
import { Session } from "./features/Session";
import { type Section, Settings } from "./features/Settings";
import {
  type Automation,
  type AutomationInput,
  automations,
} from "./shared/automations";
import { type InboxItem, inbox } from "./shared/inbox";
import {
  type AgentStatus,
  type DoctorReport,
  type LazedEvent,
  type Snapshot,
  type TerminalInfo,
  type WorkspaceInfo,
  lazed,
  subscribeEvents,
} from "./shared/lazed";
import { notificationsEnabled } from "./shared/settings";
import { safeUnlisten } from "./shared/unlisten";
import { InboxPanel } from "./widgets/Inbox";
import { InboxView } from "./widgets/InboxView";
import { Rail, type RailView } from "./widgets/Rail";
import { Sidebar } from "./widgets/Sidebar";

// daemon event names that mean "the model changed — refetch the snapshot"
const REFRESH_EVENTS = new Set([
  "project.created",
  "project.closed",
  "project.focused",
  "group.created",
  "group.updated",
  "group.removed",
  "workspace.created",
  "workspace.updated",
  "workspace.removed",
  "tab.created",
  "tab.closed",
  "terminal.created",
  "terminal.closed",
  "events.reconnect",
]);

function worst(statuses: (AgentStatus | undefined)[]): AgentStatus {
  for (const s of ["blocked", "working", "done", "idle"] as const) {
    if (statuses.includes(s)) return s;
  }
  return "unknown";
}

interface NotifyOpts {
  title: string;
  body: string;
  sound?: string;
  termId: string;
  projectId?: string;
  /** don't alert when the user is already looking at this terminal */
  skipIfFocused?: boolean;
}

/** macOS notification for an agent transition — the backend sends it via
 * notify-rust so a body click can round-trip as a `notification.jump` event
 * (the notification plugin's onAction is mobile-only). */
async function notifyAgent(o: NotifyOpts) {
  if (!notificationsEnabled()) return;
  try {
    let granted = await isPermissionGranted();
    if (!granted) granted = (await requestPermission()) === "granted";
    if (!granted) return;
    if (o.skipIfFocused && (await getCurrentWindow().isFocused())) return;
    await invoke("notify_agent", {
      termId: o.termId,
      projectId: o.projectId,
      title: o.title,
      body: o.body,
      sound: o.sound,
    });
  } catch {
    // notifications unavailable — ignore
  }
}

function basename(p?: string) {
  if (!p) return "";
  const parts = p.replace(/\/$/, "").split("/");
  return parts[parts.length - 1] || p;
}

/** The skill owns topology choices; the preamble must not impose worktrees
 * or bypass approval/readiness rules with a second orchestration recipe. */
function orchestratorPreamble(projectId: string, repoRoot: string): string {
  return [
    "You orchestrate this project. The `lazed` skill describes how to orchestrate other terminals — read it if it is not already loaded.",
    `Repo root: ${repoRoot}. Project id: ${projectId}. Your own terminal id is in $LAZED_TERM.`,
    "Use named agents through `lazed agent start/prompt/get/read/wait`. Agent start uses an existing pane and does not choose layout.",
    "Default to a sibling pane in the caller's current checkout and preserve focus. Create a worktree only when the user requests isolation or a separate branch/worktree.",
    "Use `agent prompt NAME TEXT --wait` to wait for activity and a settled state. Read output and verify changes; a settled response is not proof that tests passed.",
    "Inspect trust, login, or permission dialogs and ask the user when approval is required. Never blindly approve, resend an uncertain prompt, merge, or delete workspaces.",
  ].join("\n");
}

export function App() {
  const [snap, setSnap] = useState<Snapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [focusedTermId, setFocusedTermId] = useState<string | null>(null);
  const [focusedWsId, setFocusedWsId] = useState<string | null>(null);
  // pane zoom (⇧⌘Enter) — the focused pane fills the whole workspace area
  const [zoomed, setZoomed] = useState(false);
  const [showPicker, setShowPicker] = useState(false);
  const [showPrompt, setShowPrompt] = useState(false);
  const [showFanout, setShowFanout] = useState(false);
  const [diffWs, setDiffWs] = useState<WorkspaceInfo | null>(null);
  const [inboxOpen, setInboxOpen] = useState(false);
  const [dismissed, setDismissed] = useState<ReadonlySet<string>>(new Set());
  const [showSettings, setShowSettings] = useState(false);
  const [settingsInit, setSettingsInit] = useState<Section>("general");
  const [showSession, setShowSession] = useState(false);
  const [showAutos, setShowAutos] = useState(false);
  const [showImport, setShowImport] = useState(false);
  // project id whose "new worktree" (⌘N) sheet is open
  const [newWtProjectId, setNewWtProjectId] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  // non-null while `lazed doctor` reports missing/broken install links
  const [installReport, setInstallReport] = useState<DoctorReport | null>(null);
  // left-rail space switch — projects or the GTD inbox; automations and
  // session are full-screen overlays
  const [railView, setRailView] = useState<RailView>(() =>
    localStorage.getItem("lazed-rail") === "inbox" ? "inbox" : "projects",
  );
  const [autos, setAutos] = useState<Automation[]>([]);
  const [inboxItems, setInboxItems] = useState<InboxItem[]>([]);
  const [inboxErr, setInboxErr] = useState<string | null>(null);
  const [autoEditor, setAutoEditor] = useState<{
    auto?: Automation;
  } | null>(null);
  const refreshTimer = useRef<number | null>(null);
  const noticeTimer = useRef<number | null>(null);
  // event callbacks run outside render — mirror the bits they need
  const snapRef = useRef<Snapshot | null>(null);
  const focusRef = useRef<string | null>(null);
  // per-project workspace memory + per-workspace pane memory
  const lastWsByProject = useRef<Map<string, string>>(new Map());
  const lastTermByWs = useRef<Map<string, string>>(new Map());

  const selectRail = useCallback((v: RailView) => {
    // picking a rail view dismisses any full-screen screen — the rail
    // stays exposed over them, so a click means "show me this"
    setShowAutos(false);
    setShowSession(false);
    setShowSettings(false);
    setRailView(v);
    localStorage.setItem("lazed-rail", v);
  }, []);

  /** rail bottom buttons — toggle a full-screen screen; only one at a time */
  const toggleScreen = useCallback((s: "autos" | "session" | "settings") => {
    if (s === "settings") setSettingsInit("general");
    setShowAutos((v) => (s === "autos" ? !v : false));
    setShowSession((v) => (s === "session" ? !v : false));
    setShowSettings((v) => (s === "settings" ? !v : false));
  }, []);

  const loadAutos = useCallback(() => {
    automations
      .list()
      .then(setAutos)
      .catch(() => {});
  }, []);

  const loadInbox = useCallback(() => {
    inbox
      .list()
      .then((items) => {
        setInboxErr(null);
        setInboxItems(items);
      })
      .catch((e) => setInboxErr(String(e)));
  }, []);

  const flash = useCallback((msg: string) => {
    setNotice(msg);
    if (noticeTimer.current !== null) clearTimeout(noticeTimer.current);
    noticeTimer.current = window.setTimeout(() => setNotice(null), 5000);
  }, []);

  const runInstall = useCallback(() => {
    lazed
      .installCli()
      .then(() => lazed.installStatus())
      .then((r) => {
        if (r.ok) {
          setInstallReport(null);
          flash("lazed CLI + agent skills installed");
        } else {
          setInstallReport(r);
          setError("install incomplete — run `lazed doctor` in a terminal");
        }
      })
      .catch((e) => setError(String(e)));
  }, [flash]);

  const refresh = useCallback(() => {
    lazed
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
    lazed
      .bootstrap()
      .then((res) => {
        if (res.snapshot) setSnap(res.snapshot);
        else refresh();
      })
      .catch((e) => setError(String(e)));
    // onboarding: offer `lazed install` when the CLI/skill links are absent
    lazed
      .installStatus()
      .then((r) => setInstallReport(r.ok ? null : r))
      .catch(() => {});
    loadAutos();
    loadInbox();
    subscribeEvents((ev: LazedEvent) => {
      const name = ev.event ?? ev.type ?? "";
      if (!name) return;
      if (name === "automation.updated") {
        loadAutos();
        return;
      }
      if (name === "inbox.updated") {
        loadInbox();
        return;
      }
      if (name === "agent.status") {
        const d = ev.data ?? {};
        const termId = d.term_id as string | undefined;
        const status = d.agent_status as AgentStatus | undefined;
        if (termId && status) {
          // a terminal that goes back to work re-earns its next inbox entry
          if (status === "working" || status === "idle") {
            setDismissed((prev) => {
              if (!prev.has(termId)) return prev;
              const next = new Set(prev);
              next.delete(termId);
              return next;
            });
          }
          if (status === "blocked" || status === "done") {
            const cur = snapRef.current;
            const term = cur?.terminals.find((x) => x.term_id === termId);
            const proj = cur?.projects.find(
              (p) => p.project_id === term?.project_id,
            );
            const who =
              (d.name as string) ??
              term?.agent_name ??
              (d.agent as string) ??
              term?.agent_kind ??
              termId;
            const where = [
              proj?.label ?? basename(proj?.repo_root),
              term?.label ?? (term?.cwd ? basename(term.cwd) : undefined),
            ]
              .filter(Boolean)
              .join(" · ");
            notifyAgent({
              title:
                status === "done" ? `${who} finished` : `${who} needs input`,
              body: where || termId,
              sound: status === "done" ? "Glass" : "Ping",
              termId,
              projectId: proj?.project_id,
              skipIfFocused: focusRef.current === termId,
            });
          }
          setSnap((prev) =>
            prev
              ? {
                  ...prev,
                  terminals: prev.terminals.map((t) =>
                    t.term_id === termId
                      ? {
                          ...t,
                          agent_status: status,
                          agent_kind:
                            "agent" in d
                              ? ((d.agent as string | null) ?? undefined)
                              : t.agent_kind,
                          agent_name:
                            "name" in d
                              ? (d.name as string | null)
                              : t.agent_name,
                        }
                      : t,
                  ),
                }
              : prev,
          );
        }
        return;
      }
      if (REFRESH_EVENTS.has(name)) scheduleRefresh();
    }).catch((e) => setError(String(e)));
  }, [refresh, scheduleRefresh, loadAutos, loadInbox]);

  // while the inbox view is open, poll so expired snoozes resurface even
  // without an inbox.updated event (the daemon flips them lazily on list)
  useEffect(() => {
    if (railView !== "inbox") return;
    loadInbox();
    const t = window.setInterval(loadInbox, 30_000);
    return () => window.clearInterval(t);
  }, [railView, loadInbox]);

  const saveAutomation = useCallback(
    (input: AutomationInput) => {
      automations
        .save(input)
        .then(() => {
          setAutoEditor(null);
          loadAutos();
        })
        .catch((e) => setError(String(e)));
    },
    [loadAutos],
  );

  const projects = useMemo(() => snap?.projects ?? [], [snap]);
  const termsById = useMemo(
    () => new Map((snap?.terminals ?? []).map((t) => [t.term_id, t])),
    [snap],
  );
  // stable onClose for memoized TermViews — reading the map through a ref
  // keeps the callback identity constant across snapshot updates
  const termsByIdRef = useRef(termsById);
  termsByIdRef.current = termsById;
  const wsById = useMemo(
    () => new Map((snap?.workspaces ?? []).map((w) => [w.workspace_id, w])),
    [snap],
  );
  const tabById = useMemo(
    () => new Map((snap?.tabs ?? []).map((t) => [t.tab_id, t])),
    [snap],
  );
  const projById = useMemo(
    () => new Map(projects.map((p) => [p.project_id, p])),
    [projects],
  );

  const focusedProjectId = snap?.focused_project_id ?? projects[0]?.project_id;
  const focusedProject = focusedProjectId
    ? projById.get(focusedProjectId)
    : undefined;

  // the ⌘N sheet's target — drops itself if the project goes away
  const newWtProject = newWtProjectId
    ? projById.get(newWtProjectId)
    : undefined;

  /** The active workspace — the explicit pick when it still lives in the
   * focused project, else the remembered one, else the main checkout. */
  const activeWorkspace = useMemo(() => {
    const ids = focusedProject?.workspaces ?? [];
    const pick = (id?: string | null) =>
      id && ids.includes(id) ? wsById.get(id) : undefined;
    return (
      pick(focusedWsId) ??
      pick(lastWsByProject.current.get(focusedProjectId ?? "")) ??
      (ids.length ? wsById.get(ids[0]) : undefined)
    );
  }, [focusedProject, focusedWsId, focusedProjectId, wsById]);

  /** All panes of a workspace in display order — tab order, pane order. */
  const panesOf = useCallback(
    (ws?: WorkspaceInfo): TerminalInfo[] =>
      (ws?.tabs ?? [])
        .flatMap((tid) => tabById.get(tid)?.panes ?? [])
        .map((pid) => termsById.get(pid))
        .filter((t): t is TerminalInfo => Boolean(t)),
    [tabById, termsById],
  );

  const workspaceTabs = useMemo(
    () =>
      (activeWorkspace?.tabs ?? [])
        .map((id) => tabById.get(id))
        .filter((t): t is NonNullable<typeof t> => Boolean(t)),
    [activeWorkspace, tabById],
  );

  // the focused pane counts only while it lives inside the active workspace
  const focusedInWs =
    focusedTermId &&
    activeWorkspace &&
    termsById.get(focusedTermId)?.workspace_id === activeWorkspace.workspace_id
      ? termsById.get(focusedTermId)
      : undefined;

  const activeTabId =
    focusedInWs?.tab_id &&
    workspaceTabs.some((t) => t.tab_id === focusedInWs.tab_id)
      ? focusedInWs.tab_id
      : workspaceTabs[0]?.tab_id;

  const tabPanes = useMemo(
    () =>
      (activeTabId ? (tabById.get(activeTabId)?.panes ?? []) : [])
        .map((pid) => termsById.get(pid))
        .filter((t): t is TerminalInfo => Boolean(t)),
    [tabById, activeTabId, termsById],
  );

  const effectiveFocusedTerm =
    focusedInWs?.term_id ?? tabPanes[0]?.term_id ?? null;

  // ⇧⌘Enter zoom — the focused pane alone fills the workspace area
  const zoomedTerm =
    zoomed && effectiveFocusedTerm
      ? (termsById.get(effectiveFocusedTerm) ?? null)
      : null;

  /** every pane in the active workspace (across tabs) — prompt targets */
  const workspacePanes = useMemo(
    () => panesOf(activeWorkspace),
    [panesOf, activeWorkspace],
  );

  // sidebar display order — grouped sections first (members in group
  // order), then ungrouped projects; ⌘N targets this order
  const sidebarProjectIds = useMemo(() => {
    const groups = snap?.groups ?? [];
    const member = new Set(groups.flatMap((g) => g.projects));
    const live = new Set(projects.map((p) => p.project_id));
    return [
      ...groups.flatMap((g) => g.projects).filter((id) => live.has(id)),
      ...projects
        .filter((p) => !member.has(p.project_id))
        .map((p) => p.project_id),
    ];
  }, [snap, projects]);

  // flat workspace order as rendered in the sidebar — ⌘N and ⇧⌘[ ] step it
  const sidebarWorkspaceIds = useMemo(
    () =>
      sidebarProjectIds.flatMap((pid) => projById.get(pid)?.workspaces ?? []),
    [sidebarProjectIds, projById],
  );

  // drop zoom when there is no pane left to zoom into
  useEffect(() => {
    if (!effectiveFocusedTerm) setZoomed(false);
  }, [effectiveFocusedTerm]);

  // keep the event-callback mirrors current
  useEffect(() => {
    snapRef.current = snap;
  }, [snap]);
  useEffect(() => {
    focusRef.current = effectiveFocusedTerm;
  }, [effectiveFocusedTerm]);
  // remember the last focused pane per workspace — switching back lands on it
  useEffect(() => {
    if (effectiveFocusedTerm && activeWorkspace) {
      lastTermByWs.current.set(
        activeWorkspace.workspace_id,
        effectiveFocusedTerm,
      );
    }
  }, [effectiveFocusedTerm, activeWorkspace]);

  /** focus a workspace card — its project, remembered/first pane. */
  const selectWorkspace = useCallback(
    (wsId: string) => {
      const ws = wsById.get(wsId);
      if (!ws) return;
      setZoomed(false);
      lazed.projectFocus(ws.project_id).catch(() => {});
      lastWsByProject.current.set(ws.project_id, wsId);
      setFocusedWsId(wsId);
      const remembered = lastTermByWs.current.get(wsId);
      const panes = panesOf(ws);
      const target =
        remembered && panes.some((t) => t.term_id === remembered)
          ? remembered
          : (panes[0]?.term_id ?? null);
      setFocusedTermId(target);
    },
    [wsById, panesOf],
  );

  const selectTab = useCallback(
    (tabId: string) => {
      setZoomed(false);
      const panes = tabById.get(tabId)?.panes ?? [];
      setFocusedTermId(panes[0] ?? null);
    },
    [tabById],
  );

  const focusProject = useCallback(
    (id: string) => {
      setZoomed(false);
      lazed.projectFocus(id).catch((e) => setError(String(e)));
      const p = projById.get(id);
      const remembered = lastWsByProject.current.get(id);
      const wsId =
        p?.workspaces.find((w) => w === remembered) ?? p?.workspaces[0];
      setFocusedWsId(wsId ?? null);
      const ws = wsId ? wsById.get(wsId) : undefined;
      const panes = panesOf(ws);
      const pane =
        remembered && wsId ? lastTermByWs.current.get(wsId) : undefined;
      setFocusedTermId(
        pane && panes.some((t) => t.term_id === pane)
          ? pane
          : (panes[0]?.term_id ?? null),
      );
    },
    [projById, wsById, panesOf],
  );

  const jumpToTerm = useCallback(
    (termId: string) => {
      const t = termsById.get(termId);
      if (!t) return;
      if (t.project_id) lazed.projectFocus(t.project_id).catch(() => {});
      if (t.workspace_id) {
        if (t.project_id)
          lastWsByProject.current.set(t.project_id, t.workspace_id);
        setFocusedWsId(t.workspace_id);
      }
      setFocusedTermId(termId);
      setZoomed(false);
      setInboxOpen(false);
    },
    [termsById],
  );

  // notification click → raise the window and jump to that terminal
  useEffect(() => {
    let off: (() => void) | undefined;
    let cancelled = false;
    listen<{ term_id?: string; project_id?: string }>(
      "notification.jump",
      (e) => {
        const termId = e.payload.term_id;
        if (!termId) return;
        const win = getCurrentWindow();
        win
          .show()
          .then(() => win.unminimize())
          .then(() => win.setFocus())
          .catch(() => {});
        jumpToTerm(termId);
      },
    )
      .then((unlisten) => {
        // cleanup may have run while the listen invoke was in flight
        // (StrictMode double-mount); safeUnlisten also defers past the
        // registration eval — tauri-apps/tauri#15799
        if (cancelled) safeUnlisten(unlisten);
        else off = unlisten;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      if (off) safeUnlisten(off);
    };
  }, [jumpToTerm]);

  /** close one pane — workspace checkouts are never touched by this */
  const closeTerm = useCallback((t: TerminalInfo) => {
    setFocusedTermId((cur) => (cur === t.term_id ? null : cur));
    lazed.termClose(t.term_id).catch((e) => setError(String(e)));
  }, []);

  /** TermView's close handler — stable identity (memoized children) via
   * termsByIdRef instead of capturing the per-snapshot map. */
  const handleCloseTerm = useCallback(
    (id: string) => {
      const t = termsByIdRef.current.get(id);
      if (t) closeTerm(t);
    },
    [closeTerm],
  );

  /** close a whole workspace — kills its tabs/panes; linked checkouts get
   * `git worktree remove`d by the daemon */
  const removeWorkspace = useCallback((ws: WorkspaceInfo) => {
    setFocusedWsId((cur) => (cur === ws.workspace_id ? null : cur));
    lazed
      .workspaceRemove(ws.workspace_id, false)
      .catch((e) => setError(String(e)));
  }, []);

  /** Focus a newly created terminal once it is present in the model. */
  const focusCreatedTerm = useCallback(async (termId: string | undefined) => {
    if (!termId) return;
    // Publish the new model and selection together. Selecting an ID before
    // it exists in the snapshot falls back to the first pane, whose DOM
    // focus event can overwrite the requested selection.
    const next = await lazed.snapshot();
    setSnap(next);
    setFocusedTermId(termId);
    setZoomed(false);
  }, []);

  /** ⌘D — split a pane into the active tab (or open the workspace's first
   * tab when it has none). */
  const newPane = useCallback(() => {
    if (activeTabId) {
      lazed
        .termCreate({ tabId: activeTabId })
        .then((t) => focusCreatedTerm(t.term_id))
        .catch((e) => setError(String(e)));
    } else if (activeWorkspace) {
      lazed
        .tabCreate(activeWorkspace.workspace_id)
        .then((r) => focusCreatedTerm(r.terminal?.term_id))
        .catch((e) => setError(String(e)));
    }
  }, [activeTabId, activeWorkspace, focusCreatedTerm]);

  /** ⌘T — a fresh tab (with its first pane) in the active workspace. */
  const newTab = useCallback(() => {
    if (!activeWorkspace) return;
    lazed
      .tabCreate(activeWorkspace.workspace_id)
      .then((r) => focusCreatedTerm(r.terminal?.term_id))
      .catch((e) => setError(String(e)));
  }, [activeWorkspace, focusCreatedTerm]);

  const newProject = useCallback(
    async (cwd?: string, label?: string) => {
      setShowImport(false);
      try {
        if (cwd) {
          // one project per repo: focus instead of duplicating
          const repo = await lazed.resolveRepo(cwd).catch(() => null);
          if (repo?.repo_key) {
            const live = await lazed.snapshot().catch(() => null);
            const existing = live?.projects.find(
              (p) => p.repo_key === repo.repo_key,
            );
            if (existing) {
              focusProject(existing.project_id);
              flash(
                `already imported — focused ${existing.label ?? existing.project_id}`,
              );
              return;
            }
          }
        }
        const res = await lazed.projectCreate(cwd ?? "", label);
        const pid = res.project?.project_id;
        if (pid) await lazed.projectFocus(pid);
        const wid = res.workspace?.workspace_id;
        if (pid && wid) {
          lastWsByProject.current.set(pid, wid);
          setFocusedWsId(wid);
        }
        const tid = res.terminal?.term_id;
        if (tid) setFocusedTermId(tid);
      } catch (e) {
        setError(String(e));
      }
    },
    [focusProject, flash],
  );

  /** ⌘N — `git worktree add` for the selected project, then focus the new
   * workspace's first pane. Errors surface inside the sheet. */
  const createWorktree = useCallback(
    async (projectId: string, req: NewWorktreeRequest) => {
      const res = await lazed.workspaceCreate(
        projectId,
        req.branch,
        req.base,
        req.label,
      );
      lastWsByProject.current.set(projectId, res.workspace_id);
      await lazed.projectFocus(projectId).catch(() => {});
      setFocusedWsId(res.workspace_id);
      await focusCreatedTerm(res.terminal?.term_id);
    },
    [focusCreatedTerm],
  );

  const spawnOrchestrator = useCallback(
    async (projectId: string) => {
      const live = await lazed.snapshot().catch(() => null);
      const p = live?.projects.find((x) => x.project_id === projectId);
      if (!p) return;
      // the orchestrator lives in the main checkout workspace
      const mainWs = (live?.workspaces ?? []).find(
        (w) => w.project_id === projectId && w.is_main,
      );
      let termId =
        mainWs &&
        (live?.tabs ?? [])
          .filter((t) => t.workspace_id === mainWs.workspace_id)
          .flatMap((t) => t.panes)[0];
      if (mainWs && !termId) {
        const r = await lazed.tabCreate(mainWs.workspace_id).catch(() => null);
        termId = r?.terminal?.term_id;
      }
      if (!termId) {
        setError("orchestrator: workspace has no pane");
        return;
      }
      lazed.projectFocus(projectId).catch(() => {});
      if (mainWs) setFocusedWsId(mainWs.workspace_id);
      setFocusedTermId(termId);
      try {
        const existing = await lazed.agentGet(termId);
        if (!existing.agent) await lazed.agentStart(termId, "claude");
        else if (
          existing.agent !== "claude" ||
          !["idle", "done"].includes(existing.agent_status ?? "unknown")
        ) {
          throw new Error(
            "orchestrator: pane already has another/busy/blocked agent; inspect it first",
          );
        }
        await lazed.agentPrompt(
          termId,
          orchestratorPreamble(projectId, p.repo_root),
        );
        flash("orchestrator started");
      } catch (e) {
        setError(String(e));
      }
    },
    [flash],
  );

  const startAgent = useCallback(
    (kind: string) => {
      const target = effectiveFocusedTerm;
      setShowPicker(false);
      if (!target) return;
      lazed.agentStart(target, kind).catch((e) => setError(String(e)));
    },
    [effectiveFocusedTerm],
  );

  const submitPrompt = useCallback(
    (text: string, target: PromptTarget) => {
      const agentTerms = workspacePanes.filter((t) => t.agent_kind);
      const run = (t: TerminalInfo) =>
        (t.agent_kind
          ? lazed.agentPrompt(t.term_id, text)
          : lazed.termSend(t.term_id, `${text}\n`)
        ).catch((e) => setError(String(e)));
      if (target.kind === "focused") {
        const t = termsById.get(target.termId);
        if (t) run(t);
      } else if (target.kind === "agents") {
        for (const t of agentTerms) run(t);
      } else {
        for (const t of workspacePanes) run(t);
      }
    },
    [workspacePanes, termsById],
  );

  const doFanout = useCallback(
    async (req: FanoutRequest) => {
      setShowFanout(false);
      const repo = req.repo.trim() || focusedProject?.repo_root || "";
      if (!repo) {
        setError("fan-out: repo path is required");
        return;
      }
      let focusedNew = false;
      for (const kind of req.kinds) {
        const requestId = crypto.randomUUID();
        const branch = `${req.prefix}-${kind}-${requestId.slice(0, 8)}`;
        try {
          const res = await lazed.taskStart({
            cwd: repo,
            kind,
            branch,
            base: req.base,
            text: req.prompt,
            request_id: requestId,
            allow_dirty: req.allowDirty,
          });
          // the first new workspace becomes active; later ones spawn in the
          // background without stealing focus again
          if (!focusedNew && res.workspace_id) {
            selectWorkspace(res.workspace_id);
            focusedNew = true;
          }
          if (res.error) setError(`task ${res.task_id}: ${res.error}`);
        } catch (e) {
          setError(
            `task ${requestId}: ${String(e)}; query this ID before retrying`,
          );
        }
      }
    },
    [focusedProject, selectWorkspace],
  );

  // app-level shortcuts — registered in the capture phase so chords that
  // the terminal would otherwise consume are intercepted before the IME
  // overlay input sees them
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey || e.altKey)) return;
      // physical digit position — Option transforms e.key (⌥1 → "¡")
      const digit = /^Digit([1-9])$/.exec(e.code)?.[1];
      // a full-screen screen swallows other chords but keeps the rail
      // reachable — ⇧⌘1-4 mirror its buttons, ⌘, toggles settings off
      if (showSettings || showSession || showAutos) {
        if (e.metaKey && e.shiftKey && !e.ctrlKey && !e.altKey && digit) {
          e.preventDefault();
          e.stopPropagation();
          if (digit === "1") selectRail("projects");
          else if (digit === "2") toggleScreen("autos");
          else if (digit === "3") toggleScreen("session");
          else if (digit === "4") selectRail("inbox");
        } else if (
          e.metaKey &&
          !e.shiftKey &&
          !e.ctrlKey &&
          !e.altKey &&
          e.key === ","
        ) {
          e.preventDefault();
          e.stopPropagation();
          toggleScreen("settings");
        }
        return;
      }
      // panes of the active tab — ⌘[ ] never leaves the tab
      const paneIds = tabPanes.map((t) => t.term_id);
      const step = (list: string[], cur: string | undefined, d: number) => {
        const i = Math.max(0, list.indexOf(cur ?? ""));
        return list[(i + d + list.length) % list.length];
      };
      if (e.metaKey && !e.shiftKey && e.key === "d") {
        e.preventDefault();
        e.stopPropagation();
        newPane();
      } else if (e.metaKey && e.shiftKey && (e.key === "d" || e.key === "D")) {
        e.preventDefault();
        e.stopPropagation();
        newPane();
      } else if (e.metaKey && !e.shiftKey && e.key === "w") {
        e.preventDefault();
        e.stopPropagation();
        const t = effectiveFocusedTerm
          ? termsById.get(effectiveFocusedTerm)
          : undefined;
        if (t) closeTerm(t);
      } else if (e.metaKey && !e.shiftKey && e.key === "t") {
        e.preventDefault();
        e.stopPropagation();
        newTab();
      } else if (e.metaKey && !e.shiftKey && e.key === "n") {
        // ⌘N — new git worktree in the selected project
        e.preventDefault();
        e.stopPropagation();
        if (focusedProjectId) setNewWtProjectId(focusedProjectId);
      } else if (e.metaKey && e.shiftKey && (e.key === "n" || e.key === "N")) {
        e.preventDefault();
        e.stopPropagation();
        setShowImport(true);
      } else if (e.metaKey && e.shiftKey && e.key === "]") {
        e.preventDefault();
        e.stopPropagation();
        const next = step(
          sidebarWorkspaceIds,
          activeWorkspace?.workspace_id,
          1,
        );
        if (next) selectWorkspace(next);
      } else if (e.metaKey && e.shiftKey && e.key === "[") {
        e.preventDefault();
        e.stopPropagation();
        const prev = step(
          sidebarWorkspaceIds,
          activeWorkspace?.workspace_id,
          -1,
        );
        if (prev) selectWorkspace(prev);
      } else if (e.metaKey && !e.shiftKey && e.key === "]") {
        // ⌘] — next pane in the active tab
        e.preventDefault();
        e.stopPropagation();
        const next = step(paneIds, effectiveFocusedTerm ?? undefined, 1);
        if (next) setFocusedTermId(next);
      } else if (e.metaKey && !e.shiftKey && e.key === "[") {
        // ⌘[ — previous pane in the active tab
        e.preventDefault();
        e.stopPropagation();
        const prev = step(paneIds, effectiveFocusedTerm ?? undefined, -1);
        if (prev) setFocusedTermId(prev);
      } else if (e.metaKey && e.shiftKey && e.key === "Enter") {
        // ⇧⌘Enter — zoom the focused pane to fill the workspace (toggle)
        e.preventDefault();
        e.stopPropagation();
        if (effectiveFocusedTerm) setZoomed((z) => !z);
      } else if (e.metaKey && !e.shiftKey && e.key === "k") {
        e.preventDefault();
        e.stopPropagation();
        setShowPrompt(true);
      } else if (e.metaKey && e.shiftKey && (e.key === "a" || e.key === "A")) {
        e.preventDefault();
        e.stopPropagation();
        setShowPicker(true);
      } else if (e.metaKey && e.shiftKey && (e.key === "i" || e.key === "I")) {
        e.preventDefault();
        e.stopPropagation();
        setInboxOpen((o) => !o);
      } else if (e.metaKey && e.shiftKey && (e.key === "f" || e.key === "F")) {
        e.preventDefault();
        e.stopPropagation();
        setShowFanout(true);
      } else if (e.metaKey && !e.shiftKey && e.key === ",") {
        e.preventDefault();
        e.stopPropagation();
        setSettingsInit("general");
        setShowSettings(true);
      } else if (e.metaKey && e.shiftKey && digit === "1") {
        e.preventDefault();
        e.stopPropagation();
        selectRail("projects");
      } else if (e.metaKey && e.shiftKey && digit === "2") {
        e.preventDefault();
        e.stopPropagation();
        toggleScreen("autos");
      } else if (e.metaKey && e.shiftKey && digit === "3") {
        e.preventDefault();
        e.stopPropagation();
        toggleScreen("session");
      } else if (e.metaKey && e.shiftKey && digit === "4") {
        e.preventDefault();
        e.stopPropagation();
        selectRail("inbox");
      } else if (e.metaKey && !e.shiftKey && !e.ctrlKey && !e.altKey && digit) {
        // ⌘1-9 — jump to the Nth workspace in sidebar order
        e.preventDefault();
        e.stopPropagation();
        const wid = sidebarWorkspaceIds[Number(digit) - 1];
        if (wid) selectWorkspace(wid);
      } else if (e.ctrlKey && !e.metaKey && !e.altKey && !e.shiftKey && digit) {
        // ⌃1-9 — switch tab inside the active workspace. Unmapped digits
        // are still swallowed so the chord never reaches the PTY (⌃6
        // would otherwise send 0x1e).
        e.preventDefault();
        e.stopPropagation();
        const tab = workspaceTabs[Number(digit) - 1];
        if (tab) selectTab(tab.tab_id);
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [
    sidebarWorkspaceIds,
    tabPanes,
    workspaceTabs,
    selectTab,
    selectWorkspace,
    termsById,
    activeWorkspace,
    effectiveFocusedTerm,
    showSettings,
    showSession,
    showAutos,
    focusedProjectId,
    newPane,
    newTab,
    closeTerm,
    selectRail,
    toggleScreen,
  ]);

  const allTerms = [...termsById.values()];
  const attentionItems = allTerms
    .filter(
      (t) =>
        (t.agent_status === "blocked" || t.agent_status === "done") &&
        !dismissed.has(t.term_id),
    )
    .sort((a, b) =>
      a.agent_status === b.agent_status
        ? 0
        : a.agent_status === "blocked"
          ? -1
          : 1,
    )
    .map((term) => ({
      term,
      project:
        projects.find((p) => p.project_id === term.project_id)?.label ??
        basename(
          projects.find((p) => p.project_id === term.project_id)?.repo_root,
        ),
    }));
  const agentCount = workspacePanes.filter((t) => t.agent_kind).length;
  const openInboxCount = inboxItems.filter((i) => i.status === "open").length;

  // dock badge = undismissed attention count (blocked/finished agents plus
  // untriaged inbox items)
  useEffect(() => {
    getCurrentWindow()
      .setBadgeCount(attentionItems.length + openInboxCount || undefined)
      .catch(() => {});
  }, [attentionItems.length, openInboxCount]);

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
        <span className="title">LAZED</span>
        <span className="status">
          {error
            ? `error: ${error}`
            : notice
              ? notice
              : snap
                ? ""
                : "connecting…"}
        </span>
        <span className="tagline">STAY LAZY, ACT CRAZY</span>
      </div>
      {installReport && (
        <div className="install-banner">
          <span>
            lazed CLI + agent skills aren't linked into ~/
            {installReport.missing.length > 0 &&
              ` — missing: ${installReport.missing.join(", ")}`}
          </span>
          {installReport.warnings.map((w) => (
            <span key={w} className="warn">
              {w}
            </span>
          ))}
          <span className="grow" />
          <button type="button" onClick={runInstall}>
            Install
          </button>
          <button type="button" onClick={() => setInstallReport(null)}>
            Dismiss
          </button>
        </div>
      )}
      {inboxOpen && (
        <InboxPanel
          items={attentionItems}
          onJump={(t) => jumpToTerm(t.term_id)}
          onDismiss={(id) => setDismissed((prev) => new Set(prev).add(id))}
          onDismissAll={() =>
            setDismissed(
              (prev) =>
                new Set([
                  ...prev,
                  ...attentionItems.map((i) => i.term.term_id),
                ]),
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
          focusedTerm={effectiveFocusedTerm}
          agentCount={agentCount}
          termCount={workspacePanes.length}
          onSubmit={submitPrompt}
          onClose={() => setShowPrompt(false)}
        />
      )}
      {showFanout && (
        <Fanout
          defaultRepo={focusedProject?.repo_root ?? ""}
          onSubmit={doFanout}
          onClose={() => setShowFanout(false)}
        />
      )}
      {showSettings && (
        <Settings
          initial={settingsInit}
          onClose={() => setShowSettings(false)}
        />
      )}
      {showSession && (
        <Session
          snap={snap}
          focusedTermId={effectiveFocusedTerm}
          onJumpTerm={jumpToTerm}
          onFocusProject={focusProject}
          onClose={() => setShowSession(false)}
        />
      )}
      {showAutos && (
        <Automations
          autos={autos}
          onNew={() => setAutoEditor({})}
          onEdit={(a) => setAutoEditor({ auto: a })}
          onChanged={loadAutos}
          editorOpen={autoEditor != null}
          onClose={() => setShowAutos(false)}
          onOpenIntegrations={() => {
            setShowAutos(false);
            setSettingsInit("integrations");
            setShowSettings(true);
          }}
        />
      )}
      {newWtProject && (
        <NewWorktree
          projectName={
            newWtProject.label ??
            newWtProject.repo_root.replace(/\/$/, "").split("/").pop() ??
            newWtProject.project_id
          }
          repoRoot={newWtProject.repo_root}
          onSubmit={(req) => createWorktree(newWtProject.project_id, req)}
          onClose={() => setNewWtProjectId(null)}
        />
      )}
      {showImport && (
        <ImportProject
          onImport={newProject}
          onClose={() => setShowImport(false)}
        />
      )}
      {autoEditor && (
        <AutomationEditor
          initial={autoEditor.auto}
          projects={projects}
          onSave={saveAutomation}
          onDeleteSeen={(id) =>
            automations
              .resetSeen(id)
              .then(loadAutos)
              .catch((e) => setError(String(e)))
          }
          onClose={() => setAutoEditor(null)}
          onOpenIntegrations={() => {
            setAutoEditor(null);
            setSettingsInit("integrations");
            setShowSettings(true);
          }}
        />
      )}
      {diffWs &&
        (() => {
          const p = projById.get(diffWs.project_id);
          const agent = panesOf(diffWs).find((t) => t.agent_kind);
          return (
            <DiffView
              checkout={diffWs.path}
              repoRoot={p?.repo_root}
              label={diffWs.label ?? diffWs.branch}
              workspaceId={diffWs.workspace_id}
              agentTermId={agent?.term_id}
              onClose={() => setDiffWs(null)}
              onJump={() => {
                selectWorkspace(diffWs.workspace_id);
                setDiffWs(null);
              }}
            />
          );
        })()}
      <div className="body">
        <Rail
          active={railView}
          onSelect={selectRail}
          onAutomations={() => toggleScreen("autos")}
          onSession={() => toggleScreen("session")}
          onSettings={() => toggleScreen("settings")}
          automationsOpen={showAutos}
          sessionOpen={showSession}
          settingsOpen={showSettings}
          automationAlert={autos.some((a) => a.last_error)}
          inboxCount={openInboxCount}
        />
        {railView === "inbox" ? (
          <InboxView
            items={inboxItems}
            error={inboxErr}
            projects={projects}
            focusedProjectId={focusedProjectId}
            focusedTerm={
              effectiveFocusedTerm
                ? termsById.get(effectiveFocusedTerm)
                : undefined
            }
            onChanged={loadInbox}
            onFlash={flash}
            onError={setError}
          />
        ) : (
          <Sidebar
            snap={snap}
            focusedWorkspaceId={activeWorkspace?.workspace_id ?? null}
            focusedTermId={effectiveFocusedTerm}
            onFocusProject={focusProject}
            onFocusWorkspace={selectWorkspace}
            onJumpTerm={jumpToTerm}
            onNewProject={() => setShowImport(true)}
            onNewGroup={() =>
              lazed
                .groupCreate()
                .then((g) => g?.group_id ?? null)
                .catch((e) => {
                  setError(String(e));
                  return null;
                })
            }
            onCloseProject={(id) =>
              lazed.projectClose(id).catch((e) => setError(String(e)))
            }
            onRenameProject={(id, label) =>
              lazed.projectRename(id, label).catch((e) => setError(String(e)))
            }
            onRenameGroup={(id, label) =>
              lazed.groupRename(id, label).catch((e) => setError(String(e)))
            }
            onRemoveGroup={(id) =>
              lazed.groupRemove(id).catch((e) => setError(String(e)))
            }
            onAssignProject={(pid, gid) =>
              lazed.groupAssign(pid, gid).catch((e) => setError(String(e)))
            }
            onCloseTerm={closeTerm}
            onRemoveWorkspace={removeWorkspace}
            onDiff={setDiffWs}
            onNewWorktree={setNewWtProjectId}
            onOrchestrate={spawnOrchestrator}
          />
        )}
        <div className="main">
          {activeWorkspace ? (
            <div className="ws">
              {workspaceTabs.length > 0 && (
                <div className="ws-tabs">
                  {workspaceTabs.map((tab, i) => {
                    const panes = tab.panes
                      .map((pid) => termsById.get(pid))
                      .filter((t): t is TerminalInfo => Boolean(t));
                    const sole = panes.length === 1 ? panes[0] : undefined;
                    const name =
                      tab.label ??
                      (sole
                        ? (sole.label ??
                          sole.agent_kind ??
                          sole.branch ??
                          basename(sole.cwd))
                        : `${panes.length} panes`) ??
                      `tab ${i + 1}`;
                    return (
                      <button
                        key={tab.tab_id}
                        type="button"
                        className={`ws-tab ${activeTabId === tab.tab_id ? "active" : ""}`}
                        onClick={() => selectTab(tab.tab_id)}
                        title={`${sole?.cwd ?? tab.tab_id} (⌃${i + 1})`}
                      >
                        <span
                          className={`dot ${worst(panes.map((t) => t.agent_status))}`}
                        />
                        <span className="ws-tab-name">{name}</span>
                        {panes.length > 1 && (
                          <span className="ws-tab-count">{panes.length}</span>
                        )}
                      </button>
                    );
                  })}
                  <button
                    type="button"
                    className="ws-tab ws-tab-add"
                    title="new tab (⌘T)"
                    onClick={newTab}
                  >
                    +
                  </button>
                  {zoomedTerm && (
                    <button
                      type="button"
                      className="ws-zoom"
                      title="restore split (⇧⌘Enter)"
                      onClick={() => setZoomed(false)}
                    >
                      <HugeiconsIcon
                        icon={ArrowShrinkIcon}
                        size={11}
                        strokeWidth={1.5}
                      />
                      zoomed
                    </button>
                  )}
                </div>
              )}
              {tabPanes.length > 0 ? (
                <TermGrid
                  terms={zoomedTerm ? [zoomedTerm] : tabPanes}
                  focusedTerm={effectiveFocusedTerm}
                  onFocusTerm={setFocusedTermId}
                  onCloseTerm={handleCloseTerm}
                />
              ) : (
                <div className="empty">
                  <span>no terminals in this workspace</span>
                  <button type="button" onClick={newPane}>
                    new terminal (⌘D)
                  </button>
                  {error && <span role="alert">failed: {error}</span>}
                </div>
              )}
            </div>
          ) : (
            <div className="empty">
              {error
                ? `failed: ${error}`
                : snap
                  ? projects.length === 0
                    ? "import a project to begin — ⇧⌘N"
                    : "no panes in this workspace — ⌘T for a tab"
                  : "connecting to lazed…"}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
