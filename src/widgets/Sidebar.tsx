import type {
  AgentStatus,
  PaneInfo,
  Snapshot,
  TabInfo,
  WorkspaceInfo,
} from "../shared/herdr";

function StatusDot({ status }: { status?: AgentStatus }) {
  return (
    <span
      className={`dot ${status ?? "unknown"}`}
      title={status ?? "unknown"}
    />
  );
}

function basename(p?: string) {
  if (!p) return "";
  const parts = p.replace(/\/$/, "").split("/");
  return parts[parts.length - 1] || p;
}

function PaneRow({
  pane,
  focused,
  onClick,
}: {
  pane: PaneInfo;
  focused: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className={`side-pane ${focused ? "focused" : ""}`}
      onClick={onClick}
      title={pane.cwd ?? pane.pane_id}
    >
      <StatusDot status={pane.agent_status} />
      <span className="side-pane-name">
        {pane.display_agent ??
          pane.agent ??
          pane.title ??
          basename(pane.cwd) ??
          pane.pane_id}
      </span>
    </button>
  );
}

function TabBlock({
  tab,
  panes,
  active,
  focusedPane,
  onFocusTab,
  onFocusPane,
  onCloseTab,
}: {
  tab: TabInfo;
  panes: PaneInfo[];
  active: boolean;
  focusedPane: string | null;
  onFocusTab: (id: string) => void;
  onFocusPane: (id: string) => void;
  onCloseTab: (id: string) => void;
}) {
  return (
    <div className="side-tab">
      <div className={`side-tab-head ${active ? "active" : ""}`}>
        <button
          type="button"
          className="side-tab-label"
          onClick={() => onFocusTab(tab.tab_id)}
        >
          <StatusDot status={tab.agent_status} />
          <span>{tab.label ?? tab.tab_id}</span>
        </button>
        <button
          type="button"
          className="side-close"
          title="close tab"
          onClick={() => onCloseTab(tab.tab_id)}
        >
          ✕
        </button>
      </div>
      {active &&
        panes.map((p) => (
          <PaneRow
            key={p.pane_id}
            pane={p}
            focused={p.pane_id === focusedPane}
            onClick={() => onFocusPane(p.pane_id)}
          />
        ))}
    </div>
  );
}

export function Sidebar({
  snap,
  focusedWsId,
  activeTabId,
  focusedPane,
  onFocusWorkspace,
  onFocusTab,
  onFocusPane,
  onNewWorkspace,
  onNewTab,
  onCloseWorkspace,
  onCloseTab,
  rollup,
}: {
  snap: Snapshot | null;
  focusedWsId?: string;
  activeTabId?: string;
  focusedPane: string | null;
  onFocusWorkspace: (id: string) => void;
  onFocusTab: (id: string) => void;
  onFocusPane: (id: string) => void;
  onNewWorkspace: () => void;
  onNewTab: () => void;
  onCloseWorkspace: (id: string) => void;
  onCloseTab: (id: string) => void;
  rollup: (s: (AgentStatus | undefined)[]) => AgentStatus;
}) {
  const workspaces = snap?.workspaces ?? [];
  const tabs = snap?.tabs ?? [];
  const panes = snap?.panes ?? [];

  const paneStatus = (wsId: string): AgentStatus =>
    rollup(
      panes.filter((p) => p.workspace_id === wsId).map((p) => p.agent_status),
    );

  return (
    <div className="sidebar">
      <div className="side-scroll">
        {workspaces.map((w: WorkspaceInfo) => {
          const focused = w.workspace_id === focusedWsId;
          const wsTabs = tabs.filter((t) => t.workspace_id === w.workspace_id);
          return (
            <div key={w.workspace_id} className="side-ws">
              <div className={`side-ws-head ${focused ? "focused" : ""}`}>
                <button
                  type="button"
                  className="side-ws-label"
                  onClick={() => onFocusWorkspace(w.workspace_id)}
                >
                  <StatusDot status={paneStatus(w.workspace_id)} />
                  <span>{w.label ?? w.workspace_id}</span>
                  <span className="side-count">{w.pane_count ?? ""}</span>
                </button>
                <button
                  type="button"
                  className="side-close"
                  title="close workspace"
                  onClick={() => onCloseWorkspace(w.workspace_id)}
                >
                  ✕
                </button>
              </div>
              {focused && (
                <div className="side-tabs">
                  {wsTabs.map((t) => (
                    <TabBlock
                      key={t.tab_id}
                      tab={t}
                      panes={panes.filter((p) => p.tab_id === t.tab_id)}
                      active={t.tab_id === activeTabId}
                      focusedPane={focusedPane}
                      onFocusTab={onFocusTab}
                      onFocusPane={onFocusPane}
                      onCloseTab={onCloseTab}
                    />
                  ))}
                  <button type="button" className="side-add" onClick={onNewTab}>
                    + tab
                  </button>
                </div>
              )}
            </div>
          );
        })}
      </div>
      <div className="side-footer">
        <button type="button" className="side-add" onClick={onNewWorkspace}>
          + workspace
        </button>
      </div>
    </div>
  );
}
