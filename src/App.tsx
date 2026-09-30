import {
  ArrowShrinkIcon,
  File01Icon,
  SidebarRightIcon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ask } from "@tauri-apps/plugin-dialog";
import {
  isPermissionGranted,
  requestPermission,
} from "@tauri-apps/plugin-notification";
import {
  type ComponentProps,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { deferred } from "./components/Deferred";
import { TermGrid } from "./components/TermGrid";
import { AgentPicker } from "./features/AgentPicker";
import { DaemonUpdate } from "./features/DaemonUpdate";
import { Fanout, type FanoutRequest } from "./features/Fanout";
import { FilesPanel } from "./features/FilesPanel";
import { ImportProject } from "./features/ImportProject";
import { NewWorktree, type NewWorktreeRequest } from "./features/NewWorktree";
import { PromptBar, type PromptTarget } from "./features/PromptBar";
import type { Section } from "./features/Settings";
import {
  type Automation,
  type AutomationInput,
  automations,
} from "./shared/automations";
import { type InboxItem, inbox } from "./shared/inbox";
import {
  type AgentStatus,
  type DoctorReport,
  type FsGit,
  type LazedEvent,
  type PaneInfo,
  type Snapshot,
  type WorkspaceInfo,
  lazed,
  subscribeEvents,
} from "./shared/lazed";
import { notificationsEnabled } from "./shared/settings";
import { type TodoItem, todo } from "./shared/todo";
import { safeUnlisten } from "./shared/unlisten";
import { useSessionSnapshot } from "./shared/use-session-snapshot";
import { InboxPanel } from "./widgets/Inbox";
import { InboxView } from "./widgets/InboxView";
import { Rail, type RailView } from "./widgets/Rail";
import { Sidebar } from "./widgets/Sidebar";
import { TodoView } from "./widgets/TodoView";

const AutomationEditor = deferred<
  ComponentProps<typeof import("./features/AutomationEditor").AutomationEditor>
>(() =>
  import("./features/AutomationEditor").then((module) => ({
    default: module.AutomationEditor,
  })),
);
const Automations = deferred<
  ComponentProps<typeof import("./features/Automations").Automations>
>(() =>
  import("./features/Automations").then((module) => ({
    default: module.Automations,
  })),
);
const DiffView = deferred<
  ComponentProps<typeof import("./features/DiffView").DiffView>
>(() =>
  import("./features/DiffView").then((module) => ({
    default: module.DiffView,
  })),
);
const FileView = deferred<
  ComponentProps<typeof import("./features/FileView").FileView>
>(() =>
  import("./features/FileView").then((module) => ({
    default: module.FileView,
  })),
);
const Session = deferred<
  ComponentProps<typeof import("./features/Session").Session>
>(() =>
  import("./features/Session").then((module) => ({ default: module.Session })),
);
const Settings = deferred<
  ComponentProps<typeof import("./features/Settings").Settings>
>(() =>
  import("./features/Settings").then((module) => ({
    default: module.Settings,
  })),
);

// event names that mean "the model changed — refetch the snapshot". lazed
// emits its organization events; herdr's structural events arrive through
// the daemon's bridge as `herdr.<event>`.
const REFRESH_EVENTS = new Set([
  "project.created",
  "project.updated",
  "project.closed",
  "project.focused",
  "group.created",
  "group.updated",
  "group.removed",
  "workspace.created",
  "workspace.updated",
  "workspace.removed",
  "herdr.connected",
  "herdr.disconnected",
  "herdr.workspace_created",
  "herdr.workspace_closed",
  "herdr.workspace_renamed",
  "herdr.workspace_focused",
  "herdr.tab_created",
  "herdr.tab_closed",
  "herdr.tab_renamed",
  "herdr.tab_focused",
  "herdr.pane_created",
  "herdr.pane_closed",
  "herdr.pane_exited",
  "herdr.pane_updated",
  "herdr.pane_agent_detected",
  "herdr.layout_updated",
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
  paneId: string;
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
      paneId: o.paneId,
      projectId: o.projectId,
      title: o.title,
      body: o.body,
      sound: o.sound,
    });
  } catch {
    // notifications unavailable — ignore
  }
}

function basename(p?: string | null) {
  if (!p) return "";
  const parts = p.replace(/\/$/, "").split("/");
  return parts[parts.length - 1] || p;
}

/** The skill owns topology choices; the preamble must not impose worktrees
 * or bypass approval/readiness rules with a second orchestration recipe. */
function orchestratorPreamble(projectId: string, repoRoot: string): string {
  return [
    "You orchestrate this project. The `herdr` skill (`herdr --skill`) describes how to orchestrate other terminals — read it if not already loaded.",
    `Repo root: ${repoRoot}. lazed project id: ${projectId}. Your own herdr pane id is in $HERDR_PANE.`,
    "Use named agents through `herdr agent start/prompt/get/read/wait`. Agent start uses an existing pane and does not choose layout; `herdr pane split --current` makes one.",
    "Default to a sibling pane in the caller's current checkout and preserve focus. Create a worktree (`herdr worktree create`) only when the user requests isolation or a separate branch/worktree.",
    "Use `herdr agent prompt NAME TEXT --wait` to wait for a settled state. Read output and verify changes; a settled response is not proof that tests passed.",
    "Inspect trust, login, or permission dialogs and ask the user when approval is required. Never blindly approve, resend an uncertain prompt, merge, or delete workspaces.",
  ].join("\n");
}

export function App() {
  const {
    snap,
    patch: setSnap,
    loadSnapshot,
    invalidateSnapshot,
  } = useSessionSnapshot();
  const [error, setError] = useState<string | null>(null);
  const [connectionError, setConnectionError] = useState<string | null>(null);
  const statusError = error ?? connectionError;
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
  // right-side files panel — the active workspace's checkout tree
  const [showFiles, setShowFiles] = useState(
    () => localStorage.getItem("lazed-files") === "1",
  );
  // file tabs live in the workspace's tab strip (Orca-style) but are
  // lazed-owned surfaces — herdr never learns their ids. Scoped to the
  // checkout root they were opened from: tabs whose root no longer
  // matches are ignored at render (and restored if it comes back)
  const [fileTabs, setFileTabs] = useState<{
    root: string;
    files: { path: string; git?: FsGit | null; root: string }[];
    active: string | null;
  } | null>(null);
  // project id whose "new worktree" (⌘N) sheet is open
  const [newWtProjectId, setNewWtProjectId] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  // non-null while `lazed doctor` reports missing/broken install links
  const [installReport, setInstallReport] = useState<DoctorReport | null>(null);
  // left-rail space switch — projects, the GTD inbox, or the todo list;
  // automations and session are full-screen overlays
  const [railView, setRailView] = useState<RailView>(() => {
    const v = localStorage.getItem("lazed-rail");
    return v === "inbox" || v === "todo" ? v : "projects";
  });
  const [autos, setAutos] = useState<Automation[]>([]);
  const [inboxItems, setInboxItems] = useState<InboxItem[]>([]);
  const [inboxErr, setInboxErr] = useState<string | null>(null);
  const [todoItems, setTodoItems] = useState<TodoItem[]>([]);
  const [todoErr, setTodoErr] = useState<string | null>(null);
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

  const toggleFiles = useCallback(() => {
    setShowFiles((v) => {
      localStorage.setItem("lazed-files", v ? "0" : "1");
      return !v;
    });
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

  const loadTodo = useCallback(() => {
    todo
      .list()
      .then((items) => {
        setTodoErr(null);
        setTodoItems(items);
      })
      .catch((e) => setTodoErr(String(e)));
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
          flash("lazed CLI installed");
        } else {
          setInstallReport(r);
          setError("install incomplete — run `lazed doctor` in a terminal");
        }
      })
      .catch((e) => setError(String(e)));
  }, [flash]);

  const refresh = useCallback(() => {
    loadSnapshot()
      .then(() => setConnectionError(null))
      .catch((e) => setConnectionError(String(e)));
  }, [loadSnapshot]);

  const scheduleRefresh = useCallback(() => {
    if (refreshTimer.current !== null) return;
    refreshTimer.current = window.setTimeout(() => {
      refreshTimer.current = null;
      refresh();
    }, 40);
  }, [refresh]);

  useEffect(() => {
    let disposed = false;
    lazed
      .bootstrap()
      .then((res) => {
        if (disposed) return;
        refresh();
        if (res.herdr_error) setError(`herdr: ${res.herdr_error}`);
      })
      .catch((e) => setConnectionError(String(e)));
    // onboarding: offer `lazed install` when the CLI link is absent
    lazed
      .installStatus()
      .then((r) => {
        if (!disposed) setInstallReport(r.ok ? null : r);
      })
      .catch(() => {});
    loadAutos();
    loadInbox();
    loadTodo();
    subscribeEvents((ev: LazedEvent) => {
      if (disposed) return;
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
      if (name === "todo.updated") {
        loadTodo();
        return;
      }
      if (name === "herdr.pane.scroll_changed") {
        // scroll position lives in herdr — TermView paints its overlay
        // scrollbar from this, keyed by pane id
        window.dispatchEvent(
          new CustomEvent("lazed:pane-scroll", { detail: ev.data ?? {} }),
        );
        return;
      }
      if (name === "herdr.pane.agent_status_changed") {
        const d = ev.data ?? {};
        const paneId = d.pane_id as string | undefined;
        const status = d.agent_status as AgentStatus | undefined;
        if (paneId && status) {
          // a terminal that goes back to work re-earns its next inbox entry
          if (status === "working" || status === "idle") {
            setDismissed((prev) => {
              if (!prev.has(paneId)) return prev;
              const next = new Set(prev);
              next.delete(paneId);
              return next;
            });
          }
          if (status === "blocked" || status === "done") {
            const cur = snapRef.current;
            const term = cur?.panes.find((x) => x.pane_id === paneId);
            const ws = cur?.workspaces?.find(
              (w) => w.workspace_id === term?.workspace_id,
            );
            const proj = cur?.projects.find(
              (p) => p.project_id === ws?.project_id,
            );
            const who =
              term?.agent_name ??
              (d.display_agent as string | null) ??
              (d.agent as string | null) ??
              term?.agent ??
              paneId;
            const where = [
              proj?.label ?? basename(proj?.repo_root),
              term?.label ?? (term?.cwd ? basename(term.cwd) : undefined),
            ]
              .filter(Boolean)
              .join(" · ");
            notifyAgent({
              title:
                status === "done" ? `${who} finished` : `${who} needs input`,
              body: where || paneId,
              sound: status === "done" ? "Glass" : "Ping",
              paneId,
              projectId: proj?.project_id,
              skipIfFocused: focusRef.current === paneId,
            });
          }
          setSnap((prev) =>
            prev
              ? {
                  ...prev,
                  panes: prev.panes.map((t) =>
                    t.pane_id === paneId
                      ? {
                          ...t,
                          agent_status: status,
                          agent:
                            "agent" in d
                              ? ((d.agent as string | null) ?? undefined)
                              : t.agent,
                          display_agent:
                            "display_agent" in d
                              ? (d.display_agent as string | null)
                              : t.display_agent,
                        }
                      : t,
                  ),
                }
              : prev,
          );
        }
        return;
      }
      if (REFRESH_EVENTS.has(name)) {
        invalidateSnapshot();
        scheduleRefresh();
      }
    }).catch((e) => {
      if (!disposed) setError(String(e));
    });
    return () => {
      disposed = true;
      if (refreshTimer.current !== null)
        window.clearTimeout(refreshTimer.current);
      refreshTimer.current = null;
      if (noticeTimer.current !== null)
        window.clearTimeout(noticeTimer.current);
    };
  }, [
    refresh,
    scheduleRefresh,
    loadAutos,
    loadInbox,
    loadTodo,
    setSnap,
    invalidateSnapshot,
  ]);

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
    () => new Map((snap?.panes ?? []).map((t) => [t.pane_id, t])),
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
    (ws?: WorkspaceInfo): PaneInfo[] =>
      (ws?.tabs ?? [])
        .flatMap((tid) => tabById.get(tid)?.panes ?? [])
        .map((pid) => termsById.get(pid))
        .filter((t): t is PaneInfo => Boolean(t)),
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
        .filter((t): t is PaneInfo => Boolean(t)),
    [tabById, activeTabId, termsById],
  );

  const effectiveFocusedTerm =
    focusedInWs?.pane_id ?? tabPanes[0]?.pane_id ?? null;

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

  // the files panel/file tabs root — the active checkout, falling back to
  // the focused pane's cwd or the project root (local paths only)
  const filesRoot =
    activeWorkspace?.path ??
    (focusedInWs?.foreground_cwd || focusedInWs?.cwd) ??
    focusedProject?.repo_root;

  const shownTabs = fileTabs && fileTabs.root === filesRoot ? fileTabs : null;
  const openFiles = shownTabs?.files ?? [];
  const activeFile = shownTabs?.active ?? null;

  const openFile = useCallback(
    (path: string, git?: FsGit | null, root = filesRoot ?? "") => {
      setFileTabs((prev) => {
        const base =
          prev && filesRoot && prev.root === filesRoot
            ? prev
            : { root: filesRoot ?? "", files: [], active: null };
        return {
          ...base,
          files: base.files.some((f) => f.path === path)
            ? base.files
            : [...base.files, { path, git, root }],
          active: path,
        };
      });
    },
    [filesRoot],
  );

  const closeFile = useCallback((path: string) => {
    setFileTabs((prev) => {
      if (!prev) return prev;
      const files = prev.files.filter((f) => f.path !== path);
      return {
        ...prev,
        files,
        active:
          prev.active === path
            ? (files[files.length - 1]?.path ?? null)
            : prev.active,
      };
    });
  }, []);

  const showFile = useCallback(
    (path: string | null) =>
      setFileTabs((prev) => (prev ? { ...prev, active: path } : prev)),
    [],
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
      if (ws.project_id) {
        lazed.projectFocus(ws.project_id).catch(() => {});
        lastWsByProject.current.set(ws.project_id, wsId);
      }
      setFocusedWsId(wsId);
      const remembered = lastTermByWs.current.get(wsId);
      const panes = panesOf(ws);
      const target =
        remembered && panes.some((t) => t.pane_id === remembered)
          ? remembered
          : (panes[0]?.pane_id ?? null);
      setFocusedTermId(target);
    },
    [wsById, panesOf],
  );

  const selectTab = useCallback(
    (tabId: string) => {
      setZoomed(false);
      // a herdr tab pick puts terminals back in front — the file view
      // stays open as a tab but is no longer the shown surface
      showFile(null);
      const panes = tabById.get(tabId)?.panes ?? [];
      setFocusedTermId(panes[0] ?? null);
    },
    [tabById, showFile],
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
        pane && panes.some((t) => t.pane_id === pane)
          ? pane
          : (panes[0]?.pane_id ?? null),
      );
    },
    [projById, wsById, panesOf],
  );

  const jumpToTerm = useCallback(
    (paneId: string) => {
      const t = termsById.get(paneId);
      if (!t) return;
      const projectId = wsById.get(t.workspace_id)?.project_id;
      if (projectId) lazed.projectFocus(projectId).catch(() => {});
      if (t.workspace_id) {
        if (projectId) lastWsByProject.current.set(projectId, t.workspace_id);
        setFocusedWsId(t.workspace_id);
      }
      setFocusedTermId(paneId);
      setZoomed(false);
      setInboxOpen(false);
      // a jump must land somewhere visible — the inbox view hides the grid
      selectRail("projects");
    },
    [termsById, wsById, selectRail],
  );

  // notification click → raise the window and jump to that terminal
  useEffect(() => {
    let off: (() => void) | undefined;
    let cancelled = false;
    listen<{ pane_id?: string; project_id?: string }>(
      "notification.jump",
      (e) => {
        const paneId = e.payload.pane_id;
        if (!paneId) return;
        const win = getCurrentWindow();
        win
          .show()
          .then(() => win.unminimize())
          .then(() => win.setFocus())
          .catch(() => {});
        jumpToTerm(paneId);
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
  const closeTerm = useCallback((t: PaneInfo) => {
    setFocusedTermId((cur) => (cur === t.pane_id ? null : cur));
    lazed.paneClose(t.pane_id).catch((e) => setError(String(e)));
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

  /** close a whole workspace — herdr `worktree.remove` kills its
   * tabs/panes and deletes the linked checkout, then the branch the
   * worktree held is deleted too (herdr leaves it behind). A
   * dirty-checkout refusal escalates to an explicit force consent, which
   * also force-deletes the branch; an unmerged branch on an otherwise
   * clean removal asks once more. */
  const removeWorkspace = useCallback(async (ws: WorkspaceInfo) => {
    const clearFocus = () =>
      setFocusedWsId((cur) => (cur === ws.workspace_id ? null : cur));
    let force = false;
    for (;;) {
      try {
        await lazed.herdrCall("worktree.remove", {
          workspace_id: ws.workspace_id,
          force,
        });
        clearFocus();
        break;
      } catch (e) {
        const msg = String(e);
        if (
          !force &&
          (msg.includes("use --force") || msg.includes("modified or untracked"))
        ) {
          force = await ask(
            `"${ws.path}" has modified or untracked files. Force-remove the checkout anyway? Uncommitted work will be lost.`,
            {
              title: "Force Remove Workspace",
              kind: "warning",
              okLabel: "Force Remove",
              cancelLabel: "Cancel",
            },
          ).catch(() => false);
          if (force) continue;
        } else {
          setError(msg);
        }
        return;
      }
    }
    const branch = ws.branch;
    const repo = ws.project_id
      ? snapRef.current?.projects.find((p) => p.project_id === ws.project_id)
          ?.repo_root
      : undefined;
    if (ws.is_main || !branch || !repo) return;
    const gone = await lazed
      .branchDelete(repo, branch, force)
      .catch((e) => ({ ok: false, output: String(e) }));
    if (gone.ok) return;
    if (!force && gone.output.includes("not fully merged")) {
      const del = await ask(
        `Branch "${branch}" isn't fully merged. Delete it anyway?`,
        {
          title: "Delete Branch",
          kind: "warning",
          okLabel: "Delete Branch",
          cancelLabel: "Keep Branch",
        },
      ).catch(() => false);
      if (!del) return;
      const retry = await lazed
        .branchDelete(repo, branch, true)
        .catch((e) => ({ ok: false, output: String(e) }));
      if (retry.ok) return;
      setError(`worktree removed; branch delete failed: ${retry.output}`);
      return;
    }
    setError(`worktree removed; branch kept: ${gone.output}`);
  }, []);

  /** Focus a newly created terminal once it is present in the model. */
  const focusCreatedTerm = useCallback(
    async (paneId: string | undefined) => {
      if (!paneId) return;
      // Publish the new model and selection together. Selecting an ID before
      // it exists in the snapshot falls back to the first pane, whose DOM
      // focus event can overwrite the requested selection.
      await loadSnapshot();
      setFocusedTermId(paneId);
      setZoomed(false);
    },
    [loadSnapshot],
  );

  /** ⌘D — split next to the focused pane of the active tab (or open the
   * workspace's first tab when it has none). */
  const newPane = useCallback(() => {
    const target = effectiveFocusedTerm ?? tabPanes[0]?.pane_id;
    if (target) {
      lazed
        .paneCreate({ targetPaneId: target })
        .then((r) => focusCreatedTerm(r.pane?.pane_id))
        .catch((e) => setError(String(e)));
    } else if (activeWorkspace) {
      lazed
        .tabCreate(activeWorkspace.workspace_id)
        .then((r) => focusCreatedTerm(r.root_pane?.pane_id))
        .catch((e) => setError(String(e)));
    }
  }, [effectiveFocusedTerm, tabPanes, activeWorkspace, focusCreatedTerm]);

  /** ⌘T — a fresh tab (with its root pane) in the active workspace. */
  const newTab = useCallback(() => {
    if (!activeWorkspace) return;
    lazed
      .tabCreate(activeWorkspace.workspace_id)
      .then((r) => focusCreatedTerm(r.root_pane?.pane_id))
      .catch((e) => setError(String(e)));
  }, [activeWorkspace, focusCreatedTerm]);

  const newProject = useCallback(
    async (cwd?: string, label?: string, initSkills?: boolean) => {
      setShowImport(false);
      try {
        if (cwd) {
          // one project per repo: focus instead of duplicating
          const repo = await lazed.resolveRepo(cwd).catch(() => null);
          if (repo?.repo_key) {
            const live = await loadSnapshot().catch(() => null);
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
        const res = await lazed.projectCreate(
          cwd ?? "",
          label,
          undefined,
          initSkills,
        );
        if (res.init?.error) setError(`crew skill: ${res.init.error}`);
        else if (res.init?.installed?.length)
          flash("crew skill installed — ask any agent to use crew");
        const pid = res.project?.project_id;
        if (pid) await lazed.projectFocus(pid);
        const wid = res.workspace?.workspace_id;
        if (pid && wid) {
          lastWsByProject.current.set(pid, wid);
          setFocusedWsId(wid);
        }
        if (res.opened?.error) setError(`herdr: ${res.opened.error}`);
        const tid = res.opened?.pane_id;
        if (tid) await focusCreatedTerm(tid);
      } catch (e) {
        setError(String(e));
      }
    },
    [focusProject, flash, focusCreatedTerm, loadSnapshot],
  );

  /** ⌘N — herdr `worktree.create` on the selected project's repo, then
   * focus the new workspace's first pane. Errors surface inside the sheet. */
  const createWorktree = useCallback(
    async (projectId: string, req: NewWorktreeRequest) => {
      const repo = snapRef.current?.projects.find(
        (p) => p.project_id === projectId,
      )?.repo_root;
      if (!repo) throw new Error("unknown project");
      const res = await lazed.herdrCall<{
        workspace?: { workspace_id: string };
        root_pane?: { pane_id: string };
      }>("worktree.create", {
        cwd: repo,
        branch: req.branch,
        base: req.base,
        label: req.label,
      });
      const wsId = res.workspace?.workspace_id;
      if (wsId) {
        lastWsByProject.current.set(projectId, wsId);
        await lazed.projectFocus(projectId).catch(() => {});
        setFocusedWsId(wsId);
      }
      await focusCreatedTerm(res.root_pane?.pane_id);
    },
    [focusCreatedTerm],
  );

  const spawnOrchestrator = useCallback(
    async (projectId: string) => {
      const live = await loadSnapshot().catch(() => null);
      const p = live?.projects.find((x) => x.project_id === projectId);
      if (!p) return;
      // the orchestrator lives in the main checkout workspace
      const mainWs = (live?.workspaces ?? []).find(
        (w) => w.project_id === projectId && w.is_main,
      );
      let paneId =
        mainWs &&
        (live?.tabs ?? [])
          .filter((t) => t.workspace_id === mainWs.workspace_id)
          .flatMap((t) => t.panes)[0];
      if (mainWs && !paneId) {
        const r = await lazed.tabCreate(mainWs.workspace_id).catch(() => null);
        paneId = r?.root_pane?.pane_id;
      }
      if (!paneId) {
        setError("orchestrator: workspace has no pane");
        return;
      }
      lazed.projectFocus(projectId).catch(() => {});
      if (mainWs) setFocusedWsId(mainWs.workspace_id);
      setFocusedTermId(paneId);
      try {
        const existing = await lazed.agentGet(paneId);
        if (!existing.agent) await lazed.agentStart(paneId, "claude");
        else if (
          existing.agent !== "claude" ||
          !["idle", "done"].includes(existing.agent_status ?? "unknown")
        ) {
          throw new Error(
            "orchestrator: pane already has another/busy/blocked agent; inspect it first",
          );
        }
        await lazed.agentPrompt(
          paneId,
          orchestratorPreamble(projectId, p.repo_root),
        );
        flash("orchestrator started");
      } catch (e) {
        setError(String(e));
      }
    },
    [flash, loadSnapshot],
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
      const agentTerms = workspacePanes.filter((t) => t.agent);
      const run = (t: PaneInfo) =>
        (t.agent
          ? lazed.agentPrompt(t.pane_id, text)
          : lazed.paneSend(t.pane_id, `${text}\n`)
        ).catch((e) => setError(String(e)));
      if (target.kind === "focused") {
        const t = termsById.get(target.paneId);
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
      // ⇧⌘E — files panel toggle; a side panel, not a screen, so it stays
      // reachable even while a full-screen screen is open
      if (
        e.metaKey &&
        e.shiftKey &&
        !e.ctrlKey &&
        !e.altKey &&
        (e.key === "e" || e.key === "E")
      ) {
        e.preventDefault();
        e.stopPropagation();
        toggleFiles();
        return;
      }
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
          else if (digit === "5") selectRail("todo");
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
      const paneIds = tabPanes.map((t) => t.pane_id);
      const step = (list: string[], cur: string | undefined, d: number) => {
        const i = Math.max(0, list.indexOf(cur ?? ""));
        return list[(i + d + list.length) % list.length];
      };
      // workspace chords belong to the projects space — on the inbox/todo
      // rails a bound key is still swallowed (an unhandled ⌘W would close
      // the window, ⌃N would reach the PTY) but fires nothing until those
      // views get chords of their own
      const onProjects = railView === "projects";
      if (e.metaKey && !e.shiftKey && e.key === "d") {
        e.preventDefault();
        e.stopPropagation();
        if (onProjects) newPane();
      } else if (e.metaKey && e.shiftKey && (e.key === "d" || e.key === "D")) {
        e.preventDefault();
        e.stopPropagation();
        if (onProjects) newPane();
      } else if (e.metaKey && !e.shiftKey && e.key === "w") {
        e.preventDefault();
        e.stopPropagation();
        if (onProjects && activeFile) {
          // a file tab is showing — ⌘W closes it, not the terminal
          closeFile(activeFile);
          return;
        }
        const t = effectiveFocusedTerm
          ? termsById.get(effectiveFocusedTerm)
          : undefined;
        if (onProjects && t) closeTerm(t);
      } else if (e.metaKey && !e.shiftKey && e.key === "t") {
        e.preventDefault();
        e.stopPropagation();
        if (onProjects) newTab();
      } else if (e.metaKey && !e.shiftKey && e.key === "n") {
        // ⌘N — new git worktree in the selected project
        e.preventDefault();
        e.stopPropagation();
        if (onProjects && focusedProjectId) setNewWtProjectId(focusedProjectId);
      } else if (e.metaKey && e.shiftKey && (e.key === "n" || e.key === "N")) {
        e.preventDefault();
        e.stopPropagation();
        if (onProjects) setShowImport(true);
      } else if (e.metaKey && e.shiftKey && e.key === "]") {
        e.preventDefault();
        e.stopPropagation();
        const next = step(
          sidebarWorkspaceIds,
          activeWorkspace?.workspace_id,
          1,
        );
        if (onProjects && next) selectWorkspace(next);
      } else if (e.metaKey && e.shiftKey && e.key === "[") {
        e.preventDefault();
        e.stopPropagation();
        const prev = step(
          sidebarWorkspaceIds,
          activeWorkspace?.workspace_id,
          -1,
        );
        if (onProjects && prev) selectWorkspace(prev);
      } else if (e.metaKey && !e.shiftKey && e.key === "]") {
        // ⌘] — next pane in the active tab
        e.preventDefault();
        e.stopPropagation();
        const next = step(paneIds, effectiveFocusedTerm ?? undefined, 1);
        if (onProjects && next) setFocusedTermId(next);
      } else if (e.metaKey && !e.shiftKey && e.key === "[") {
        // ⌘[ — previous pane in the active tab
        e.preventDefault();
        e.stopPropagation();
        const prev = step(paneIds, effectiveFocusedTerm ?? undefined, -1);
        if (onProjects && prev) setFocusedTermId(prev);
      } else if (e.metaKey && e.shiftKey && e.key === "Enter") {
        // ⇧⌘Enter — zoom the focused pane to fill the workspace (toggle)
        e.preventDefault();
        e.stopPropagation();
        if (onProjects && effectiveFocusedTerm) setZoomed((z) => !z);
      } else if (e.metaKey && !e.shiftKey && e.key === "k") {
        e.preventDefault();
        e.stopPropagation();
        if (onProjects) setShowPrompt(true);
      } else if (e.metaKey && e.shiftKey && (e.key === "a" || e.key === "A")) {
        e.preventDefault();
        e.stopPropagation();
        if (onProjects) setShowPicker(true);
      } else if (e.metaKey && e.shiftKey && (e.key === "i" || e.key === "I")) {
        // ⇧⌘I — the attention panel floats above any rail view
        e.preventDefault();
        e.stopPropagation();
        setInboxOpen((o) => !o);
      } else if (e.metaKey && e.shiftKey && (e.key === "f" || e.key === "F")) {
        e.preventDefault();
        e.stopPropagation();
        if (onProjects) setShowFanout(true);
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
      } else if (e.metaKey && e.shiftKey && digit === "5") {
        e.preventDefault();
        e.stopPropagation();
        selectRail("todo");
      } else if (e.metaKey && !e.shiftKey && !e.ctrlKey && !e.altKey && digit) {
        // ⌘1-9 — jump to the Nth workspace in sidebar order
        e.preventDefault();
        e.stopPropagation();
        const wid = sidebarWorkspaceIds[Number(digit) - 1];
        if (onProjects && wid) selectWorkspace(wid);
      } else if (e.ctrlKey && !e.metaKey && !e.altKey && !e.shiftKey && digit) {
        // ⌃1-9 — switch tab inside the active workspace. Unmapped digits
        // are still swallowed so the chord never reaches the PTY (⌃6
        // would otherwise send 0x1e).
        e.preventDefault();
        e.stopPropagation();
        const tab = workspaceTabs[Number(digit) - 1];
        if (onProjects && tab) selectTab(tab.tab_id);
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
    railView,
    focusedProjectId,
    newPane,
    newTab,
    closeTerm,
    selectRail,
    toggleScreen,
    toggleFiles,
    activeFile,
    closeFile,
  ]);

  const allTerms = useMemo(() => [...termsById.values()], [termsById]);
  const attentionItems = useMemo(
    () =>
      allTerms
        .filter(
          (t) =>
            (t.agent_status === "blocked" || t.agent_status === "done") &&
            !dismissed.has(t.pane_id),
        )
        .sort((a, b) =>
          a.agent_status === b.agent_status
            ? 0
            : a.agent_status === "blocked"
              ? -1
              : 1,
        )
        .map((term) => {
          const pid = wsById.get(term.workspace_id)?.project_id;
          const proj = pid ? projById.get(pid) : undefined;
          return { term, project: proj?.label ?? basename(proj?.repo_root) };
        }),
    [allTerms, dismissed, wsById, projById],
  );
  const agentCount = workspacePanes.filter((t) => t.agent).length;
  const openInboxCount = useMemo(
    () => inboxItems.filter((i) => i.status === "open").length,
    [inboxItems],
  );
  const openTodoCount = useMemo(
    () => todoItems.filter((i) => !i.done).length,
    [todoItems],
  );
  // stable term list for the memoized TermGrid — [zoomedTerm] would be a
  // fresh array every render otherwise
  const gridTerms = useMemo(
    () => (zoomedTerm ? [zoomedTerm] : tabPanes),
    [zoomedTerm, tabPanes],
  );

  // ── stable props for memoized children (Sidebar/Rail/InboxPanel) ──
  const openImport = useCallback(() => setShowImport(true), []);
  const openAutomations = useCallback(
    () => toggleScreen("autos"),
    [toggleScreen],
  );
  const openSession = useCallback(
    () => toggleScreen("session"),
    [toggleScreen],
  );
  const openSettings = useCallback(
    () => toggleScreen("settings"),
    [toggleScreen],
  );
  const createGroup = useCallback(
    () =>
      lazed
        .groupCreate()
        .then((g) => g?.group_id ?? null)
        .catch((e) => {
          setError(String(e));
          return null;
        }),
    [],
  );
  const closeProjectById = useCallback((id: string) => {
    lazed.projectClose(id).catch((e) => setError(String(e)));
  }, []);
  const renameProject = useCallback((id: string, label: string) => {
    lazed.projectRename(id, label).catch((e) => setError(String(e)));
  }, []);
  const renameGroupById = useCallback((id: string, label: string) => {
    lazed.groupRename(id, label).catch((e) => setError(String(e)));
  }, []);
  const removeGroupById = useCallback((id: string) => {
    lazed.groupRemove(id).catch((e) => setError(String(e)));
  }, []);
  const assignProjectGroup = useCallback((pid: string, gid: string | null) => {
    lazed.groupAssign(pid, gid).catch((e) => setError(String(e)));
  }, []);
  const jumpToAttention = useCallback(
    (t: PaneInfo) => jumpToTerm(t.pane_id),
    [jumpToTerm],
  );
  const dismissAttention = useCallback(
    (id: string) => setDismissed((prev) => new Set(prev).add(id)),
    [],
  );
  const dismissAllAttention = useCallback(
    () =>
      setDismissed(
        (prev) =>
          new Set([...prev, ...attentionItems.map((i) => i.term.pane_id)]),
      ),
    [attentionItems],
  );
  const closeInbox = useCallback(() => setInboxOpen(false), []);

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
          {statusError
            ? `error: ${statusError}`
            : notice
              ? notice
              : snap
                ? ""
                : "connecting…"}
        </span>
        <span className="tagline">STAY LAZY, ACT CRAZY</span>
        <button
          type="button"
          className={`titlebar-btn ${showFiles ? "sel" : ""}`}
          title="files panel (⇧⌘E)"
          onClick={toggleFiles}
        >
          <HugeiconsIcon icon={SidebarRightIcon} size={14} strokeWidth={1.5} />
        </button>
      </div>
      <DaemonUpdate />
      {installReport && (
        <div className="install-banner">
          <span>
            lazed CLI isn't linked into ~/
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
          onJump={jumpToAttention}
          onDismiss={dismissAttention}
          onDismissAll={dismissAllAttention}
          onClose={closeInbox}
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
          const p = projById.get(diffWs.project_id ?? "");
          const agent = panesOf(diffWs).find((t) => t.agent);
          return (
            <DiffView
              checkout={diffWs.path ?? ""}
              repoRoot={p?.repo_root}
              label={diffWs.label ?? diffWs.branch}
              workspaceId={diffWs.workspace_id}
              agentTermId={agent?.pane_id}
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
          onAutomations={openAutomations}
          onSession={openSession}
          onSettings={openSettings}
          automationsOpen={showAutos}
          sessionOpen={showSession}
          settingsOpen={showSettings}
          automationAlert={autos.some((a) => a.last_error)}
          inboxCount={openInboxCount}
          todoCount={openTodoCount}
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
        ) : railView === "todo" ? (
          <TodoView
            items={todoItems}
            error={todoErr}
            onChanged={loadTodo}
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
            onNewProject={openImport}
            onNewGroup={createGroup}
            onCloseProject={closeProjectById}
            onRenameProject={renameProject}
            onRenameGroup={renameGroupById}
            onRemoveGroup={removeGroupById}
            onAssignProject={assignProjectGroup}
            onCloseTerm={closeTerm}
            onRemoveWorkspace={removeWorkspace}
            onDiff={setDiffWs}
            onNewWorktree={setNewWtProjectId}
            onOrchestrate={spawnOrchestrator}
          />
        )}
        {railView === "projects" && (
          <div className="main">
            {activeWorkspace ? (
              <div className="ws">
                {(workspaceTabs.length > 0 || openFiles.length > 0) && (
                  <div className="ws-tabs">
                    {workspaceTabs.map((tab, i) => {
                      const panes = tab.panes
                        .map((pid) => termsById.get(pid))
                        .filter((t): t is PaneInfo => Boolean(t));
                      const sole = panes.length === 1 ? panes[0] : undefined;
                      const name =
                        tab.label ??
                        (sole
                          ? (sole.label ??
                            sole.agent_name ??
                            sole.agent ??
                            basename(sole.cwd))
                          : `${panes.length} panes`) ??
                        `tab ${i + 1}`;
                      return (
                        <button
                          key={tab.tab_id}
                          type="button"
                          className={`ws-tab ${!activeFile && activeTabId === tab.tab_id ? "active" : ""}`}
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
                    {/* file tabs — lazed surfaces, never herdr tabs */}
                    {openFiles.map((f) => (
                      <div
                        key={f.path}
                        className={`ws-tab ws-file-tab ${f.git && f.git !== "!" ? `s-${f.git}` : ""} ${activeFile === f.path ? "active" : ""}`}
                      >
                        <button
                          type="button"
                          className="ws-file-open"
                          title={f.path}
                          onClick={() => showFile(f.path)}
                        >
                          <HugeiconsIcon
                            icon={File01Icon}
                            size={11}
                            strokeWidth={1.5}
                          />
                          <span className="ws-tab-name">
                            {basename(f.path)}
                          </span>
                        </button>
                        <button
                          type="button"
                          className="ws-file-x"
                          title="close file (⌘W)"
                          onClick={() => closeFile(f.path)}
                        >
                          ✕
                        </button>
                      </div>
                    ))}
                  </div>
                )}
                <div className="ws-body">
                  {tabPanes.length > 0 ? (
                    <TermGrid
                      terms={gridTerms}
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
                  {activeFile && filesRoot && (
                    <FileView
                      root={
                        openFiles.find((f) => f.path === activeFile)?.root ??
                        filesRoot
                      }
                      path={activeFile}
                      git={openFiles.find((f) => f.path === activeFile)?.git}
                      onClose={() => closeFile(activeFile)}
                    />
                  )}
                </div>
              </div>
            ) : (
              <div className="empty">
                {statusError
                  ? `failed: ${statusError}`
                  : snap
                    ? projects.length === 0
                      ? "import a project to begin — ⇧⌘N"
                      : "no panes in this workspace — ⌘T for a tab"
                    : "connecting to lazed…"}
              </div>
            )}
          </div>
        )}
        {showFiles && railView === "projects" && (
          <FilesPanel
            root={filesRoot}
            label={
              activeWorkspace
                ? (activeWorkspace.label ??
                  activeWorkspace.branch ??
                  basename(activeWorkspace.path))
                : undefined
            }
            branch={activeWorkspace?.branch}
            paneCount={workspacePanes.length}
            onClose={() => setShowFiles(false)}
            onOpenDiff={
              activeWorkspace ? () => setDiffWs(activeWorkspace) : undefined
            }
            onOpenFile={openFile}
          />
        )}
      </div>
    </div>
  );
}
