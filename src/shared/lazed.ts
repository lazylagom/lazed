import { Channel, invoke } from "@tauri-apps/api/core";

export type AgentStatus = "working" | "blocked" | "done" | "idle" | "unknown";

/** A group = a named, ordered collection of projects (sidebar section). */
export interface GroupInfo {
  group_id: string;
  label?: string;
  /** ordered project ids — a project sits in at most one group */
  projects: string[];
}

/**
 * A project = one git repo (lazed's overlay on herdr). Owns the ordered list
 * of herdr workspace ids checked out from that repo — index 0 is the main
 * checkout.
 */
export interface ProjectInfo {
  project_id: string;
  label?: string;
  repo_root: string;
  repo_key: string;
  /** id of the group this project belongs to, if any */
  group_id?: string | null;
  /** ordered herdr workspace ids — index 0 is the main checkout */
  workspaces: string[];
  focused?: boolean;
}

/**
 * A herdr workspace (`w1`) with lazed's annotation merged in: which project
 * it belongs to, its checkout path/branch, main vs linked worktree.
 * `tabs` is derived client-side from the tab list (see `normalize`).
 */
export interface WorkspaceInfo {
  workspace_id: string;
  /** lazed project — null for a herdr workspace lazed couldn't file */
  project_id?: string | null;
  label?: string;
  /** checkout path (repo_root for the main workspace) */
  path?: string;
  /** branch at open — the sidebar card's display name for worktrees */
  branch?: string;
  is_main?: boolean;
  // herdr WorkspaceInfo
  number?: number;
  focused?: boolean;
  pane_count?: number;
  tab_count?: number;
  active_tab_id?: string;
  agent_status?: AgentStatus;
  /** ordered tab ids (derived) */
  tabs: string[];
}

/** A herdr tab (`w1:t1`). `panes` is derived client-side. */
export interface TabInfo {
  tab_id: string;
  workspace_id: string;
  label?: string;
  number?: number;
  focused?: boolean;
  pane_count?: number;
  agent_status?: AgentStatus;
  /** ordered pane ids (derived — layout order when herdr reports one) */
  panes: string[];
}

/** A herdr pane (`w1:p1`) = one PTY owned by herdr. */
export interface PaneInfo {
  pane_id: string;
  workspace_id: string;
  tab_id: string;
  terminal_id?: string;
  cwd?: string | null;
  foreground_cwd?: string | null;
  label?: string | null;
  title?: string | null;
  /** agent kind herdr detected in the pane (manifest name), if any */
  agent?: string | null;
  display_agent?: string | null;
  agent_status?: AgentStatus;
  /** live herdr agent name — merged from `agents[]` by `normalize` */
  agent_name?: string | null;
  focused?: boolean;
  revision?: number;
  scroll?: {
    offset_from_bottom: number;
    max_offset_from_bottom: number;
    viewport_rows: number;
  } | null;
}

/** herdr AgentInfo — a pane that hosts a live (possibly named) agent. */
export interface AgentInfo {
  pane_id: string;
  workspace_id?: string;
  tab_id?: string;
  name?: string | null;
  agent?: string | null;
  display_agent?: string | null;
  agent_status?: AgentStatus;
  launch_pending?: boolean;
  interactive_ready?: boolean;
}

export interface Snapshot {
  focused_project_id?: string;
  groups?: GroupInfo[];
  projects: ProjectInfo[];
  workspaces?: WorkspaceInfo[];
  tabs?: TabInfo[];
  panes: PaneInfo[];
  agents?: AgentInfo[];
  layouts?: {
    tab_id: string;
    workspace_id: string;
    panes: { pane_id: string; focused?: boolean }[];
  }[];
  focused_workspace_id?: string | null;
  focused_tab_id?: string | null;
  focused_pane_id?: string | null;
  /** execution layer — false means herdr is unreachable (degraded mode) */
  herdr?: { connected: boolean; session?: string; version?: string | null };
}

/**
 * Fill in the client-side derivations the daemon's merged snapshot leaves
 * out: `workspace.tabs`, `tab.panes` (layout order when available) and each
 * pane's live agent name. Idempotent — safe on an already-normalized value.
 */
export function normalize(snap: Snapshot): Snapshot {
  const panes = snap.panes ?? [];
  const tabs = snap.tabs ?? [];
  const workspaces = snap.workspaces ?? [];
  const byNumber = <T extends { number?: number }>(a: T, b: T) =>
    (a.number ?? 0) - (b.number ?? 0);
  const layoutOrder = new Map<string, string[]>();
  for (const l of snap.layouts ?? []) {
    layoutOrder.set(
      l.tab_id,
      l.panes.map((p) => p.pane_id),
    );
  }
  const nameByPane = new Map<string, string>();
  for (const a of snap.agents ?? []) {
    if (a.name) nameByPane.set(a.pane_id, a.name);
  }
  const normPanes = panes.map((p) => ({
    ...p,
    agent_name: nameByPane.get(p.pane_id) ?? p.agent_name ?? null,
  }));
  // one pass over panes/tabs — a per-tab filter is O(panes×tabs)
  const panesByTab = new Map<string, string[]>();
  for (const p of normPanes) {
    const list = panesByTab.get(p.tab_id);
    if (list) list.push(p.pane_id);
    else panesByTab.set(p.tab_id, [p.pane_id]);
  }
  const normTabs = [...tabs].sort(byNumber).map((t) => {
    const inTab = panesByTab.get(t.tab_id) ?? [];
    const order = layoutOrder.get(t.tab_id);
    if (!order) return { ...t, panes: inTab };
    const inTabSet = new Set(inTab);
    const orderSet = new Set(order);
    return {
      ...t,
      panes: [
        ...order.filter((id) => inTabSet.has(id)),
        ...inTab.filter((id) => !orderSet.has(id)),
      ],
    };
  });
  const tabsByWs = new Map<string, string[]>();
  for (const t of normTabs) {
    const list = tabsByWs.get(t.workspace_id);
    if (list) list.push(t.tab_id);
    else tabsByWs.set(t.workspace_id, [t.tab_id]);
  }
  const normWs = workspaces.map((w) => ({
    ...w,
    tabs: tabsByWs.get(w.workspace_id) ?? [],
  }));
  return { ...snap, panes: normPanes, tabs: normTabs, workspaces: normWs };
}

/** `session.status` result — daemon liveness + version + object counts. */
export interface SessionStatus {
  running: boolean;
  version?: string;
  /** canonical path of the running daemon binary */
  exe?: string;
  pid?: number;
  /** unix seconds when the daemon process started */
  started_at?: number;
  /** the binary on disk is newer than the running process — restart to pick it up */
  binary_updated?: boolean;
  capabilities?: string[];
  workspaces?: number;
  projects?: number;
  herdr?: { session?: string; bridge_connected?: boolean };
}

/** `herdr.status` — adapter + herdr server health. */
export interface HerdrStatus {
  installed: boolean;
  binary?: string | null;
  session: string;
  expected_version: string;
  running: boolean;
  version?: string | null;
  socket?: string | null;
  compat_warning?: string | null;
  bridge_connected: boolean;
  error?: string | null;
}

export interface LazedEvent {
  /** daemon event name, e.g. "herdr.pane_created", "project.created" */
  event?: string;
  /** synthetic client-side events also carry `type` (e.g. "events.reconnect") */
  type?: string;
  data?: Record<string, unknown>;
}

export interface BootstrapResult {
  snapshot?: Snapshot;
  /** herdr could not be started — the app runs in degraded mode */
  herdr_error?: string | null;
}

/** One row of `lazed doctor --json` — a path the install manages. */
export interface DoctorCheck {
  id: string;
  path?: string;
  status: string;
  detail?: string;
}

/** `lazed doctor --json` — `missing` holds check ids that need install. */
export interface DoctorReport {
  ok: boolean;
  missing: string[];
  warnings: string[];
  checks: DoctorCheck[];
}

// ── files panel ────────────────────────────────────────────────────

/** git badge on a tree entry: M modified, D deleted, U untracked, ! ignored. */
export type FsGit = "M" | "D" | "U" | "!";

export interface FsEntry {
  /** repo-relative path, "/" separators */
  path: string;
  kind: "file" | "dir";
  git?: FsGit | null;
  /** dir the backend didn't enumerate — ignored dir or embedded repo;
   * row is informational, not expandable */
  collapsed?: boolean;
}

export interface FsTree {
  /** resolved root (repo toplevel) the entries are relative to */
  root: string;
  git: boolean;
  truncated?: boolean;
  entries: FsEntry[];
}

export interface FsRead {
  content?: string;
  truncated?: boolean;
  binary?: boolean;
}

export interface FsHit {
  path: string;
  line: number;
  text: string;
}

export const lazed = {
  bootstrap: () => invoke<BootstrapResult>("bootstrap"),
  installStatus: () => invoke<DoctorReport>("install_status"),
  installCli: () => invoke("install_cli"),
  snapshot: () => invoke<Snapshot>("session_snapshot"),
  status: () => invoke<SessionStatus>("session_status"),
  herdrStatus: () => invoke<HerdrStatus>("herdr_status"),
  /** stop + respawn the lazed daemon; herdr (and every pane) is untouched */
  restartServer: (onlyIfEmpty = false) =>
    invoke<SessionStatus>("server_restart", { onlyIfEmpty }),

  // panes (herdr) — the frame stream itself lives in TermView
  paneClose: (paneId: string) => invoke("pane_close", { paneId }),
  /**
   * New pane. `targetPaneId` splits next to an existing pane (same tab);
   * `workspaceId` opens a fresh tab with its root pane.
   */
  paneCreate: (opts: {
    targetPaneId?: string;
    workspaceId?: string;
    cwd?: string;
  }) => invoke<{ pane: PaneInfo | null; tab?: TabInfo }>("pane_create", opts),
  /** type a line into a shell pane (text + Enter) */
  paneSend: (paneId: string, text: string) =>
    invoke("pane_send", { paneId, text }),
  paneRead: (paneId: string, lines?: number) =>
    invoke<{ text: string }>("pane_read", { paneId, lines }),

  // projects
  projectCreate: (
    cwd: string,
    label?: string,
    groupId?: string,
    initSkills?: boolean,
  ) =>
    invoke<{
      project: ProjectInfo;
      workspace?: WorkspaceInfo | null;
      opened?: {
        workspace_id?: string;
        tab_id?: string;
        pane_id?: string;
        error?: string;
      } | null;
      /** crew skill install result (`initSkills`) */
      init?: {
        installed?: string[];
        skipped?: string[];
        error?: string;
      } | null;
    }>("project_create", { cwd, label, groupId, initSkills }),
  projectFocus: (projectId: string) => invoke("project_focus", { projectId }),
  projectClose: (projectId: string) => invoke("project_close", { projectId }),
  projectRename: (projectId: string, label: string) =>
    invoke("project_rename", { projectId, label }),

  // groups — named project collections (Orca-style sidebar sections)
  groupCreate: (label?: string) => invoke<GroupInfo>("group_create", { label }),
  groupRename: (groupId: string, label: string) =>
    invoke("group_rename", { groupId, label }),
  groupRemove: (groupId: string) => invoke("group_remove", { groupId }),
  /** groupId null/undefined ungroups the project. */
  groupAssign: (projectId: string, groupId?: string | null) =>
    invoke("group_assign", { projectId, groupId: groupId ?? null }),

  /**
   * Generic herdr API call straight to the herdr socket — worktree
   * create/remove and any other execution-layer method; lazed keeps no
   * duplicate of what herdr already provides.
   */
  herdrCall: <T = Record<string, unknown>>(
    method: string,
    params?: Record<string, unknown>,
  ) => invoke<T>("herdr_call", { method, params: params ?? {} }),
  workspaceRename: (workspaceId: string, label: string) =>
    invoke("workspace_rename", { workspaceId, label }),

  // tabs (herdr) — each opens with a root pane
  tabCreate: (workspaceId: string, label?: string) =>
    invoke<{ tab: TabInfo; root_pane: PaneInfo }>("tab_create", {
      workspaceId,
      label,
    }),
  tabClose: (tabId: string) => invoke("tab_close", { tabId }),

  // git helpers (pure — not tied to workspace entities)
  worktreeDiff: (checkout: string, base?: string) =>
    invoke<{
      branch: string;
      base?: string;
      diff: string;
      stat: string;
      untracked: string[];
    }>("worktree_diff", { checkout, base }),
  worktreeMerge: (repo: string, branch: string) =>
    invoke<{ ok: boolean; output: string }>("worktree_merge", {
      repo,
      branch,
    }),
  /** delete the branch a removed worktree was on (herdr leaves it behind);
   * `force` maps to `git branch -D` */
  branchDelete: (repo: string, branch: string, force?: boolean) =>
    invoke<{ ok: boolean; output: string }>("branch_delete", {
      repo,
      branch,
      force: force ?? false,
    }),
  resolveRepo: (cwd: string) =>
    invoke<{ repo_key: string; repo_root?: string; name?: string } | null>(
      "resolve_repo",
      { cwd },
    ),

  // files panel — checkout tree, peek, search (local fs, git-aware)
  fsWatch: (root: string) =>
    invoke<{ watch_id: string; root: string }>("fs_watch", { root }),
  fsUnwatch: (watchId: string) => invoke("fs_unwatch", { watchId }),
  fsTree: (root: string) => invoke<FsTree>("fs_tree", { root }),
  fsRead: (path: string) => invoke<FsRead>("fs_read", { path }),
  fsSearch: (root: string, query: string) =>
    invoke<{ results: FsHit[]; truncated?: boolean }>("fs_search", {
      root,
      query,
    }),
  fsDiff: (root: string, path: string) =>
    invoke<{ diff: string }>("fs_diff", { root, path }),
  fsOpen: (path: string) => invoke("open_path", { path }),

  // agents (herdr's) + lazed tasks
  taskStart: (params: {
    cwd: string;
    kind: string;
    text: string;
    branch: string;
    base?: string;
    request_id: string;
    allow_dirty?: boolean;
  }) =>
    invoke<{
      task_id: string;
      workspace_id?: string;
      pane_id?: string;
      agent_name?: string;
      phase: string;
      error?: string;
      receipt?: { accepted?: boolean; submitted?: boolean } | null;
    }>("task_start", { params }),
  agentStart: (paneId: string, kind: string) =>
    invoke("agent_start", { paneId, kind }),
  agentPrompt: (paneId: string, text: string) =>
    invoke("agent_prompt", { paneId, text }),
  agentGet: (paneId: string) => invoke<AgentInfo>("agent_get", { paneId }),
  agentDetect: () => invoke<AgentDetectResult>("agent_detect"),
};

/** One agent kind's install probe result from `agent_detect`. */
export interface DetectedAgent {
  kind: string;
  /** resolved executable path — absent means not installed */
  path?: string;
}

export interface AgentDetectResult {
  context: string;
  agents: DetectedAgent[];
}

/** Agent kinds offered in the picker — a subset of herdr's manifests. */
export const AGENT_KINDS = [
  "claude",
  "codex",
  "devin",
  "gemini",
  "opencode",
  "pi",
  "cursor",
  "copilot",
] as const;

export function subscribeEvents(handler: (ev: LazedEvent) => void) {
  const ch = new Channel<LazedEvent>();
  ch.onmessage = handler;
  return invoke("subscribe_events", { onEvent: ch });
}
