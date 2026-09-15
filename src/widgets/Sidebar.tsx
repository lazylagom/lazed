import {
  AppWindowIcon,
  BotIcon,
  FolderImportIcon,
  FolderLibraryIcon,
  GitBranchIcon,
  TerminalIcon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { useEffect, useRef, useState } from "react";
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
      <HugeiconsIcon
        icon={pane.agent ? BotIcon : TerminalIcon}
        size={12}
        strokeWidth={1.5}
        className="side-ico"
      />
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

function RenameInput({
  initial,
  onCommit,
  onCancel,
}: {
  initial: string;
  onCommit: (label: string) => void;
  onCancel: () => void;
}) {
  const [value, setValue] = useState(initial);
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    ref.current?.focus();
    ref.current?.select();
  }, []);
  return (
    <input
      ref={ref}
      className="side-rename"
      value={value}
      onChange={(e) => setValue(e.target.value)}
      onKeyDown={(e) => {
        if (e.key === "Enter") onCommit(value.trim());
        if (e.key === "Escape") onCancel();
      }}
      onBlur={onCancel}
    />
  );
}

interface RenameTarget {
  kind: "ws" | "tab";
  id: string;
  label: string;
}

function TabBlock({
  tab,
  panes,
  active,
  focusedPane,
  renaming,
  onFocusTab,
  onFocusPane,
  onCloseTab,
  onRenameTab,
  onRenameStart,
  onRenameCancel,
}: {
  tab: TabInfo;
  panes: PaneInfo[];
  active: boolean;
  focusedPane: string | null;
  renaming: RenameTarget | null;
  onFocusTab: (id: string) => void;
  onFocusPane: (id: string) => void;
  onCloseTab: (id: string) => void;
  onRenameTab: (id: string, label: string) => void;
  onRenameStart: (t: RenameTarget) => void;
  onRenameCancel: () => void;
}) {
  return (
    <div className="side-tab">
      <div className={`side-tab-head ${active ? "active" : ""}`}>
        {renaming?.kind === "tab" && renaming.id === tab.tab_id ? (
          <RenameInput
            initial={renaming.label}
            onCommit={(l) => {
              if (l) onRenameTab(tab.tab_id, l);
              onRenameCancel();
            }}
            onCancel={onRenameCancel}
          />
        ) : (
          <button
            type="button"
            className="side-tab-label"
            onClick={() => onFocusTab(tab.tab_id)}
            onDoubleClick={() =>
              onRenameStart({
                kind: "tab",
                id: tab.tab_id,
                label: tab.label ?? "",
              })
            }
            title="double-click to rename"
          >
            <StatusDot status={tab.agent_status} />
            <HugeiconsIcon
              icon={AppWindowIcon}
              size={12}
              strokeWidth={1.5}
              className="side-ico"
            />
            <span>{tab.label ?? tab.tab_id}</span>
          </button>
        )}
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
  onRenameWorkspace,
  onRenameTab,
  onDiff,
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
  onRenameWorkspace: (id: string, label: string) => void;
  onRenameTab: (id: string, label: string) => void;
  onDiff: (ws: WorkspaceInfo) => void;
  rollup: (s: (AgentStatus | undefined)[]) => AgentStatus;
}) {
  const [renaming, setRenaming] = useState<RenameTarget | null>(null);
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
                {renaming?.kind === "ws" && renaming.id === w.workspace_id ? (
                  <RenameInput
                    initial={renaming.label}
                    onCommit={(l) => {
                      if (l) onRenameWorkspace(w.workspace_id, l);
                      setRenaming(null);
                    }}
                    onCancel={() => setRenaming(null)}
                  />
                ) : (
                  <button
                    type="button"
                    className="side-ws-label"
                    onClick={() => onFocusWorkspace(w.workspace_id)}
                    onDoubleClick={() =>
                      setRenaming({
                        kind: "ws",
                        id: w.workspace_id,
                        label: w.label ?? "",
                      })
                    }
                    title="double-click to rename"
                  >
                    <StatusDot status={paneStatus(w.workspace_id)} />
                    <HugeiconsIcon
                      icon={FolderLibraryIcon}
                      size={13}
                      strokeWidth={1.5}
                      className="side-ico"
                    />
                    <span>{w.label ?? w.workspace_id}</span>
                    <span className="side-count">{w.pane_count ?? ""}</span>
                  </button>
                )}
                {w.worktree?.is_linked_worktree && (
                  <button
                    type="button"
                    className="side-close"
                    title={`diff ${w.worktree.checkout_path}`}
                    onClick={() => onDiff(w)}
                  >
                    <HugeiconsIcon
                      icon={GitBranchIcon}
                      size={11}
                      strokeWidth={1.5}
                    />
                  </button>
                )}
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
                      renaming={renaming}
                      onFocusTab={onFocusTab}
                      onFocusPane={onFocusPane}
                      onCloseTab={onCloseTab}
                      onRenameTab={onRenameTab}
                      onRenameStart={setRenaming}
                      onRenameCancel={() => setRenaming(null)}
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
          <HugeiconsIcon
            icon={FolderImportIcon}
            size={12}
            strokeWidth={1.5}
            className="side-ico"
          />
          import project
        </button>
      </div>
    </div>
  );
}
