import { Channel, invoke } from "@tauri-apps/api/core";

export type AgentStatus = "working" | "blocked" | "done" | "idle" | "unknown";

/** A group = a named, ordered collection of projects (sidebar section). */
export interface GroupInfo {
  group_id: string;
  label?: string;
  /** ordered project ids — a project sits in at most one group */
  projects: string[];
}

/** A project = one git repo. `terminals[0]` is the first terminal (root checkout). */
export interface ProjectInfo {
  project_id: string;
  label?: string;
  repo_root: string;
  repo_key: string;
  /** id of the group this project belongs to, if any */
  group_id?: string | null;
  terminals: string[];
  focused?: boolean;
}

/** One terminal = one PTY owned by the daemon. */
export interface TerminalInfo {
  term_id: string;
  cwd: string;
  command?: string;
  label?: string;
  /** "worktree" | "plain" */
  kind?: string;
  /** git branch of the checkout at spawn — the tab's display name */
  branch?: string;
  cols?: number;
  rows?: number;
  dead?: boolean;
  agent_kind?: string;
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
  terminals: TerminalInfo[];
}

/** `session.status` result — daemon liveness + version + object counts. */
export interface SessionStatus {
  running: boolean;
  version?: string;
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

export const lazed = {
  bootstrap: () => invoke<BootstrapResult>("bootstrap"),
  snapshot: () => invoke<Snapshot>("session_snapshot"),
  status: () => invoke<SessionStatus>("session_status"),

  // terminal control streams
  termClose: (termId: string) => invoke("term_close", { termId }),
  termCreate: (projectId: string, command?: string, label?: string) =>
    invoke<TerminalInfo>("term_create", { projectId, command, label }),
  termSend: (termId: string, text: string) =>
    invoke("term_send", { termId, text }),
  termRead: (termId: string, lines?: number) =>
    invoke<{ text: string }>("term_read", { termId, lines }),

  // projects
  projectCreate: (cwd: string, label?: string, groupId?: string) =>
    invoke<{
      project: ProjectInfo;
      terminal: { term_id: string };
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

  // worktrees
  worktreeCreate: (
    projectId: string,
    branch: string,
    base?: string,
    label?: string,
  ) =>
    invoke<{
      checkout_path: string;
      branch: string;
      project_id: string;
      terminal: TerminalInfo;
    }>("worktree_create", { projectId, branch, base, label }),
  worktreeRemove: (termId: string, force = false) =>
    invoke("worktree_remove", { termId, force }),
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
