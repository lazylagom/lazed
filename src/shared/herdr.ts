import { Channel, invoke } from "@tauri-apps/api/core";

export type AgentStatus = "working" | "blocked" | "done" | "idle" | "unknown";

export interface WorkspaceInfo {
  workspace_id: string;
  label?: string;
  number?: number;
  focused?: boolean;
  active_tab_id?: string;
  agent_status?: AgentStatus;
  pane_count?: number;
  tab_count?: number;
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
  data?: Record<string, unknown>;
  id?: string;
  result?: unknown;
  error?: unknown;
}

export const herdr = {
  bootstrap: () => invoke<Record<string, unknown>>("bootstrap"),
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
  tabCreate: (workspaceId: string, cwd?: string) =>
    invoke("tab_create", { workspaceId, cwd }),
  tabFocus: (tabId: string) => invoke("tab_focus", { tabId }),
  tabClose: (tabId: string) => invoke("tab_close", { tabId }),
};

export function subscribeEvents(handler: (ev: HerdrEvent) => void) {
  const ch = new Channel<HerdrEvent>();
  ch.onmessage = handler;
  return invoke("subscribe_events", { onEvent: ch });
}
