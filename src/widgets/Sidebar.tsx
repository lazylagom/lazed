import {
  AppWindowIcon,
  ArrowDown01Icon,
  ArrowRight01Icon,
  BotIcon,
  FolderGitIcon,
  FolderImportIcon,
  FolderLibraryIcon,
  GitBranchIcon,
  PlusIcon,
  TerminalIcon,
  TowerControlIcon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { useEffect, useRef, useState } from "react";
import type {
  AgentStatus,
  PaneInfo,
  RepoRef,
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

/** Deterministic project accent — Orca gives each repo a colored glyph. */
function projectHue(key: string) {
  let h = 0;
  for (const c of key) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return h % 360;
}

function Caret({ open }: { open: boolean }) {
  return (
    <HugeiconsIcon
      icon={open ? ArrowDown01Icon : ArrowRight01Icon}
      size={11}
      strokeWidth={1.5}
      className="side-caret"
    />
  );
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
            <Caret open={active} />
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

/** A workspace row + its tabs — used standalone and inside a project group. */
function WorkspaceBlock({
  w,
  role,
  focused,
  wsTabs,
  panes,
  activeTabId,
  focusedPane,
  renaming,
  onFocusWorkspace,
  onFocusTab,
  onFocusPane,
  onCloseWorkspace,
  onCloseTab,
  onRenameWorkspace,
  onRenameTab,
  onDiff,
  onRenameStart,
  onRenameCancel,
  status,
}: {
  w: WorkspaceInfo;
  /** "tower" = the project's repo-root workspace; "worker" = linked worktree */
  role?: "tower" | "worker";
  focused: boolean;
  wsTabs: TabInfo[];
  panes: PaneInfo[];
  activeTabId?: string;
  focusedPane: string | null;
  renaming: RenameTarget | null;
  onFocusWorkspace: (id: string) => void;
  onFocusTab: (id: string) => void;
  onFocusPane: (id: string) => void;
  onCloseWorkspace: (id: string) => void;
  onCloseTab: (id: string) => void;
  onRenameWorkspace: (id: string, label: string) => void;
  onRenameTab: (id: string, label: string) => void;
  onDiff: (ws: WorkspaceInfo) => void;
  onRenameStart: (t: RenameTarget) => void;
  onRenameCancel: () => void;
  status: AgentStatus;
}) {
  return (
    <div className="side-ws">
      <div className={`side-ws-head ${focused ? "focused" : ""}`}>
        {renaming?.kind === "ws" && renaming.id === w.workspace_id ? (
          <RenameInput
            initial={renaming.label}
            onCommit={(l) => {
              if (l) onRenameWorkspace(w.workspace_id, l);
              onRenameCancel();
            }}
            onCancel={onRenameCancel}
          />
        ) : (
          <button
            type="button"
            className="side-ws-label"
            onClick={() => onFocusWorkspace(w.workspace_id)}
            onDoubleClick={() =>
              onRenameStart({
                kind: "ws",
                id: w.workspace_id,
                label: w.label ?? "",
              })
            }
            title="double-click to rename"
          >
            <Caret open={focused} />
            <StatusDot status={status} />
            <HugeiconsIcon
              icon={role === "tower" ? FolderLibraryIcon : GitBranchIcon}
              size={13}
              strokeWidth={1.5}
              className="side-ico"
            />
            <span className="side-ws-name">{w.label ?? w.workspace_id}</span>
            {role === "tower" && <span className="side-pill">primary</span>}
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
            <HugeiconsIcon icon={GitBranchIcon} size={11} strokeWidth={1.5} />
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
              onRenameStart={onRenameStart}
              onRenameCancel={onRenameCancel}
            />
          ))}
        </div>
      )}
    </div>
  );
}

export function Sidebar({
  snap,
  repoRefs,
  focusedWsId,
  activeTabId,
  focusedPane,
  onFocusWorkspace,
  onFocusTab,
  onFocusPane,
  onNewWorkspace,
  onCloseWorkspace,
  onCloseTab,
  onRenameWorkspace,
  onRenameTab,
  onDiff,
  onTower,
  rollup,
}: {
  snap: Snapshot | null;
  repoRefs: Map<string, RepoRef>;
  focusedWsId?: string;
  activeTabId?: string;
  focusedPane: string | null;
  onFocusWorkspace: (id: string) => void;
  onFocusTab: (id: string) => void;
  onFocusPane: (id: string) => void;
  onNewWorkspace: () => void;
  onCloseWorkspace: (id: string) => void;
  onCloseTab: (id: string) => void;
  onRenameWorkspace: (id: string, label: string) => void;
  onRenameTab: (id: string, label: string) => void;
  onDiff: (ws: WorkspaceInfo) => void;
  onTower: (repoKey: string) => void;
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

  const rootCwdOf = (wsId: string) => {
    const p = panes.find((p) => p.workspace_id === wsId);
    return p?.cwd ?? p?.foreground_cwd;
  };

  // project grouping: workspaces sharing a repo_key belong to the same
  // repo. repoRefs merges herdr's worktree stamp with our own resolution
  // (fresh imports have no stamp until the first worktree op).
  const groups = new Map<string, WorkspaceInfo[]>();
  for (const w of workspaces) {
    const key = repoRefs.get(w.workspace_id)?.repo_key;
    if (!key) continue;
    const arr = groups.get(key) ?? [];
    arr.push(w);
    groups.set(key, arr);
  }

  const blockProps = {
    panes,
    activeTabId,
    focusedPane,
    renaming,
    onFocusWorkspace,
    onFocusTab,
    onFocusPane,
    onCloseWorkspace,
    onCloseTab,
    onRenameWorkspace,
    onRenameTab,
    onDiff,
    onRenameStart: setRenaming,
    onRenameCancel: () => setRenaming(null),
  };

  const renderWs = (w: WorkspaceInfo, role?: "tower" | "worker") => (
    <WorkspaceBlock
      key={w.workspace_id}
      w={w}
      role={role}
      focused={w.workspace_id === focusedWsId}
      wsTabs={tabs.filter((t) => t.workspace_id === w.workspace_id)}
      status={paneStatus(w.workspace_id)}
      {...blockProps}
    />
  );

  const rendered = new Set<string>();
  const rows: React.ReactNode[] = [];

  for (const w of workspaces) {
    if (rendered.has(w.workspace_id)) continue;
    const key = repoRefs.get(w.workspace_id)?.repo_key;
    const members = key ? groups.get(key) : undefined;
    if (!members) {
      rendered.add(w.workspace_id);
      rows.push(renderWs(w));
      continue;
    }
    for (const m of members) rendered.add(m.workspace_id);
    // tower = the repo's own checkout: herdr says is_linked_worktree=false;
    // for un-stamped workspaces fall back to root cwd == repo_root
    const ref = repoRefs.get(members[0].workspace_id);
    const tower =
      members.find((m) => m.worktree?.is_linked_worktree === false) ??
      members.find((m) => rootCwdOf(m.workspace_id) === ref?.repo_root) ??
      members[0];
    const workers = members.filter((m) => m !== tower);
    const name = ref?.name ?? basename(ref?.repo_root) ?? "project";
    rows.push(
      <div key={key} className="side-proj">
        <div className="side-proj-head">
          <button
            type="button"
            className="side-proj-label"
            onClick={() => onFocusWorkspace(tower.workspace_id)}
            title={ref?.repo_root ?? name}
          >
            <HugeiconsIcon
              icon={FolderGitIcon}
              size={13}
              strokeWidth={1.5}
              className="side-proj-ico"
              color={`hsl(${projectHue(key ?? name)} 65% 62%)`}
            />
            <span>{name}</span>
            <span className="side-count">{members.length}</span>
          </button>
          {key && (
            <button
              type="button"
              className="side-close"
              title="spawn control tower agent"
              onClick={() => onTower(key)}
            >
              <HugeiconsIcon
                icon={TowerControlIcon}
                size={12}
                strokeWidth={1.5}
              />
            </button>
          )}
        </div>
        <div className="side-proj-children">
          {renderWs(tower, "tower")}
          {workers.map((m) => renderWs(m, "worker"))}
        </div>
      </div>,
    );
  }

  return (
    <div className="sidebar">
      <div className="side-head">
        <span className="side-head-label">Projects</span>
        <button
          type="button"
          className="side-head-btn"
          title="import project"
          onClick={onNewWorkspace}
        >
          <HugeiconsIcon icon={PlusIcon} size={13} strokeWidth={1.5} />
        </button>
      </div>
      <div className="side-scroll">{rows}</div>
      <div className="side-footer">
        <button
          type="button"
          className="side-foot-btn"
          title="import project"
          onClick={onNewWorkspace}
        >
          <HugeiconsIcon icon={FolderImportIcon} size={14} strokeWidth={1.5} />
        </button>
      </div>
    </div>
  );
}
