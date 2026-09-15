import { Channel, invoke } from "@tauri-apps/api/core";

export type AgentStatus = "working" | "blocked" | "done" | "idle" | "unknown";

export interface WorktreeInfo {
  checkout_path: string;
  is_linked_worktree: boolean;
  repo_key?: string;
  repo_name?: string;
  repo_root?: string;
}

export interface WorkspaceInfo {
  workspace_id: string;
  label?: string;
  number?: number;
  focused?: boolean;
  active_tab_id?: string;
  agent_status?: AgentStatus;
  pane_count?: number;
  tab_count?: number;
  worktree?: WorktreeInfo;
}

export interface TabInfo {
  tab_id: string;
  workspace_id: string;
  label?: string;
  number?: number;
  focused?: boolean;
  agent_status?: AgentStatus;
  pane_count?: number;
}

export interface PaneInfo {
  pane_id: string;
  tab_id: string;
  workspace_id: string;
  agent_status?: AgentStatus;
  agent?: string;
  display_agent?: string;
  title?: string;
  cwd?: string;
  foreground_cwd?: string;
  focused?: boolean;
}

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface LayoutSplit {
  id: string;
  direction: "right" | "down";
  ratio: number;
  rect: Rect;
}

export interface TabLayout {
  tab_id: string;
  workspace_id: string;
  area: Rect;
  focused_pane_id?: string;
  zoomed?: boolean;
  panes: { pane_id: string; rect: Rect; focused?: boolean }[];
  splits: LayoutSplit[];
}

export interface AgentInfo {
  agent: string;
  name?: string;
  pane_id: string;
  agent_status?: AgentStatus;
  title?: string;
  terminal_title?: string;
  interactive_ready?: boolean;
}

export interface Snapshot {
  workspaces: WorkspaceInfo[];
  tabs: TabInfo[];
  panes: PaneInfo[];
  layouts: TabLayout[];
  agents: AgentInfo[];
  focused_workspace_id?: string;
  focused_tab_id?: string;
  focused_pane_id?: string;
}

export interface HerdrEvent {
  /** event name, e.g. "pane_created", "pane_agent_status_changed" */
  event?: string;
  /** synthetic client-side events also carry `type` (e.g. "events.reconnect") */
  type?: string;
  data?: Record<string, unknown>;
  id?: string;
  result?: unknown;
  error?: unknown;
}

export interface BootstrapResult {
  workspace?: unknown;
  panes?: unknown[];
  /** set when the live herdr server fails the compat/version check */
  herdr_warning?: string;
}

export const herdr = {
  bootstrap: () => invoke<BootstrapResult>("bootstrap"),
  snapshot: () => invoke<Snapshot>("session_snapshot"),
  splitPane: (paneId: string, direction: "right" | "down") =>
    invoke("split_pane", { paneId, direction }),
  closePane: (paneId: string) => invoke("close_pane", { paneId }),
  resizePane: (paneId: string, direction: string, amount: number) =>
    invoke("resize_pane", { paneId, direction, amount }),
  runInPane: (paneId: string, command: string) =>
    invoke("run_in_pane", { paneId, command }),
  workspaceCreate: (cwd?: string, label?: string) =>
    invoke("workspace_create", { cwd, label }),
  workspaceFocus: (workspaceId: string) =>
    invoke("workspace_focus", { workspaceId }),
  workspaceClose: (workspaceId: string) =>
    invoke("workspace_close", { workspaceId }),
  workspaceRename: (workspaceId: string, label: string) =>
    invoke("workspace_rename", { workspaceId, label }),
  tabCreate: (workspaceId: string, cwd?: string) =>
    invoke("tab_create", { workspaceId, cwd }),
  tabFocus: (tabId: string) => invoke("tab_focus", { tabId }),
  tabClose: (tabId: string) => invoke("tab_close", { tabId }),
  tabRename: (tabId: string, label: string) =>
    invoke("tab_rename", { tabId, label }),
  agentStart: (paneId: string, kind: string, name?: string) =>
    invoke("agent_start", { paneId, kind, name }),
  agentPrompt: (paneId: string, text: string) =>
    invoke("agent_prompt", { paneId, text }),
  agentGet: (paneId: string) => invoke<AgentInfo>("agent_get", { paneId }),
  paneSendText: (paneId: string, text: string) =>
    invoke("pane_send_text", { paneId, text }),
  worktreeCreate: (
    cwd: string,
    branch?: string,
    base?: string,
    label?: string,
    workspace?: string,
  ) => invoke("worktree_create", { cwd, branch, base, label, workspace }),
  worktreeList: (cwd?: string) => invoke("worktree_list", { cwd }),
  worktreeRemove: (workspaceId: string, force = false) =>
    invoke("worktree_remove", { workspaceId, force }),
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
  remoteConnect: (target: string, session?: string) =>
    invoke("remote_connect", { target, session }),
  remoteDisconnect: () => invoke("remote_disconnect"),
  remoteStatus: () =>
    invoke<{ target?: string; session?: string }>("remote_status"),
  machineList: () => invoke<MachineInfo[]>("machine_list"),
  machineRemove: (id: string) => invoke("machine_remove", { id }),
  machineRename: (id: string, label: string) =>
    invoke("machine_rename", { id, label }),
};

/** POSIX single-quote escaping for commands sent through `pane run`. */
export function shQuote(s: string): string {
  return `'${s.replace(/'/g, `'\\''`)}'`;
}

/** A saved herdr SSH machine profile (`herdr machine list --json`). */
export interface MachineInfo {
  id: string;
  label: string;
  target: string;
  /** remote session name — "default" needs no --session flag */
  session: string;
  enabled: boolean;
  selected: boolean;
}

/** herdr-supported agent kinds (from `agent start --kind`). */
export const AGENT_KINDS = [
  "claude",
  "codex",
  "gemini",
  "opencode",
  "cursor",
  "devin",
  "pi",
  "copilot",
  "amp",
  "grok",
  "cline",
  "agy",
  "droid",
  "kimi",
  "kiro",
  "omp",
  "hermes",
  "kilo",
  "muse",
  "qwen",
  "qodercli",
  "mastracode",
  "maki",
] as const;

export function subscribeEvents(handler: (ev: HerdrEvent) => void) {
  const ch = new Channel<HerdrEvent>();
  ch.onmessage = handler;
  return invoke("subscribe_events", { onEvent: ch });
}
