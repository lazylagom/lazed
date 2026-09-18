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
 * A project = one git repo. Owns workspaces; `workspaces` is ordered —
 * the first entry is the main-checkout workspace.
 */
export interface ProjectInfo {
  project_id: string;
  label?: string;
  repo_root: string;
  repo_key: string;
  /** id of the group this project belongs to, if any */
  group_id?: string | null;
  /** ordered workspace ids — index 0 is the main checkout */
  workspaces: string[];
  focused?: boolean;
}

/** One workspace = one checkout (the main checkout or a linked worktree). */
export interface WorkspaceInfo {
  workspace_id: string;
  project_id: string;
  label?: string;
  /** checkout path (repo_root for the main workspace) */
  path: string;
  /** branch at spawn — the sidebar card's display name */
  branch?: string;
  is_main: boolean;
  /** ordered tab ids */
  tabs: string[];
}

/** A tab inside a workspace — an ordered row of panes. */
export interface TabInfo {
  tab_id: string;
  workspace_id: string;
  label?: string;
  /** ordered pane (terminal) ids */
  panes: string[];
}

/** One terminal = one pane = one PTY owned by the daemon. */
export interface TerminalInfo {
  term_id: string;
  /** owning tab / workspace / project ids */
  tab_id?: string;
  workspace_id?: string;
  project_id?: string;
  cwd: string;
  command?: string;
  label?: string;
  /** "worktree" | "plain" — display tag derived from the workspace */
  kind?: string;
  /** git branch of the checkout at spawn */
  branch?: string;
  cols?: number;
  rows?: number;
  dead?: boolean;
  agent_kind?: string;
  agent_name?: string | null;
  agent_status?: AgentStatus;
  scroll?: {
    offset_from_bottom: number;
    max_offset_from_bottom: number;
    viewport_rows: number;
  };
}

export interface Snapshot {
  focused_project_id?: string;
  groups?: GroupInfo[];
  projects: ProjectInfo[];
  workspaces?: WorkspaceInfo[];
  tabs?: TabInfo[];
  terminals: TerminalInfo[];
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
  terms?: number;
  projects?: number;
}

export interface LazedEvent {
  /** daemon event name, e.g. "terminal.created", "agent.status" */
  event?: string;
  /** synthetic client-side events also carry `type` (e.g. "events.reconnect") */
  type?: string;
  data?: Record<string, unknown>;
}

export interface BootstrapResult {
  snapshot?: Snapshot;
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

export const lazed = {
  bootstrap: () => invoke<BootstrapResult>("bootstrap"),
  installStatus: () => invoke<DoctorReport>("install_status"),
  installCli: () => invoke("install_cli"),
  snapshot: () => invoke<Snapshot>("session_snapshot"),
  status: () => invoke<SessionStatus>("session_status"),
  /** stop + respawn the daemon; panes come back from the persisted session */
  restartServer: () => invoke<SessionStatus>("server_restart"),

  // pane control streams
  termClose: (termId: string) => invoke("term_close", { termId }),
  /**
   * New pane. `tabId` splits a pane into an existing tab; `workspaceId` /
   * `projectId` open a fresh tab under them (with its first pane).
   */
  termCreate: (opts: {
    tabId?: string;
    workspaceId?: string;
    projectId?: string;
    command?: string;
    label?: string;
  }) => invoke<TerminalInfo>("term_create", opts),
  termSend: (termId: string, text: string) =>
    invoke("term_send", { termId, text }),
  termRead: (termId: string, lines?: number) =>
    invoke<{ text: string }>("term_read", { termId, lines }),

  // projects
  projectCreate: (cwd: string, label?: string, groupId?: string) =>
    invoke<{
      project: ProjectInfo;
      workspace: WorkspaceInfo;
      terminal: TerminalInfo;
    }>("project_create", { cwd, label, groupId }),
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

  // workspaces — one checkout each (main or a linked git worktree)
  workspaceCreate: (
    projectId: string,
    branch: string,
    base?: string,
    label?: string,
  ) =>
    invoke<{
      checkout_path: string;
      branch: string;
      project_id: string;
      workspace_id: string;
      tab_id: string;
      terminal: TerminalInfo;
    }>("workspace_create", { projectId, branch, base, label }),
  workspaceRemove: (workspaceId: string, force = false) =>
    invoke("workspace_remove", { workspaceId, force }),
  workspaceRename: (workspaceId: string, label: string) =>
    invoke("workspace_rename", { workspaceId, label }),

  // tabs — a named pane row inside a workspace
  tabCreate: (workspaceId: string, label?: string, command?: string) =>
    invoke<{ tab_id: string; terminal: TerminalInfo }>("tab_create", {
      workspaceId,
      label,
      command,
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
  resolveRepo: (cwd: string) =>
    invoke<{ repo_key: string; repo_root?: string; name?: string } | null>(
      "resolve_repo",
      { cwd },
    ),

  // agents
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
      term_id?: string;
      phase: string;
      error?: string;
    }>("task_start", { params }),
  agentStart: (termId: string, kind: string) =>
    invoke("agent_start", { termId, kind }),
  agentPrompt: (termId: string, text: string) =>
    invoke("agent_prompt", { termId, text }),
  agentGet: (termId: string) =>
    invoke<{
      term_id: string;
      agent?: string;
      agent_status?: AgentStatus;
    }>("agent_get", { termId }),
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

/** POSIX single-quote escaping for shell command strings. */
export function shQuote(s: string): string {
  return `'${s.replace(/'/g, `'\\''`)}'`;
}

/** Agent kinds the daemon knows how to start. */
export const AGENT_KINDS = [
  "claude",
  "codex",
  "antigravity",
  "devin",
  "gemini",
  "opencode",
  "aider",
  "pi",
] as const;

export function subscribeEvents(handler: (ev: LazedEvent) => void) {
  const ch = new Channel<LazedEvent>();
  ch.onmessage = handler;
  return invoke("subscribe_events", { onEvent: ch });
}
