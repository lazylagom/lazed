import {
  ArrowDown01Icon,
  ArrowRight01Icon,
  BotIcon,
  Delete01Icon,
  Folder01Icon,
  FolderPlusIcon,
  GitBranchIcon,
  GitCompareIcon,
  GroupLayersIcon,
  MoreHorizontalIcon,
  PencilEdit01Icon,
  PlusIcon,
  SparklesIcon,
  TerminalIcon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { ask } from "@tauri-apps/plugin-dialog";
import { useEffect, useRef, useState } from "react";
import type {
  AgentStatus,
  GroupInfo,
  ProjectInfo,
  Snapshot,
  TerminalInfo,
  WorkspaceInfo,
} from "../shared/lazed";
import { useSidebarWidth } from "./sidebar-width";

function statusClass(s?: AgentStatus) {
  return `st-${s ?? "unknown"}`;
}

function basename(p?: string) {
  if (!p) return "";
  const parts = p.replace(/\/$/, "").split("/");
  return parts[parts.length - 1] || p;
}

function confirmClose(title: string, message: string) {
  return ask(message, {
    title,
    kind: "warning",
    okLabel: "Close",
    cancelLabel: "Cancel",
  }).catch(() => false);
}

const COLLAPSED_KEY = "sidebar-collapsed-groups";

function loadCollapsed(): ReadonlySet<string> {
  try {
    const v = JSON.parse(localStorage.getItem(COLLAPSED_KEY) ?? "[]");
    return new Set(
      Array.isArray(v) ? v.filter((x) => typeof x === "string") : [],
    );
  } catch {
    return new Set();
  }
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

/** A pane row — one terminal inside a workspace's tab row. */
function PaneRow({
  t,
  focused,
  onFocus,
  onClose,
}: {
  t: TerminalInfo;
  focused: boolean;
  onFocus: () => void;
  onClose: () => void;
}) {
  const name =
    t.label ?? t.agent_kind ?? t.branch ?? basename(t.cwd) ?? t.term_id;
  const close = async () => {
    const ok = await confirmClose(
      "Close Pane",
      `Close pane “${name}”? Its shell will be killed.`,
    );
    if (ok) onClose();
  };
  return (
    <div className={`side-ws-head ${focused ? "focused" : ""}`}>
      <button
        type="button"
        className="side-ws-label"
        onClick={onFocus}
        title={t.cwd}
      >
        <span className={`dot ${t.agent_status ?? "unknown"}`} />
        <HugeiconsIcon
          icon={t.agent_kind ? BotIcon : TerminalIcon}
          size={12}
          strokeWidth={1.5}
          className="side-ico"
        />
        <span className={`side-ws-name ${statusClass(t.agent_status)}`}>
          {name}
        </span>
      </button>
      <button
        type="button"
        className="side-close"
        title="close pane"
        onClick={close}
      >
        ✕
      </button>
    </div>
  );
}

/** A workspace card under a project — one checkout with its tabs' panes. */
function WorkspaceRow({
  ws,
  panes,
  focusedWs,
  focusedTermId,
  onFocusWorkspace,
  onJumpTerm,
  onCloseTerm,
  onRemoveWorkspace,
  onDiff,
}: {
  ws: WorkspaceInfo;
  panes: TerminalInfo[];
  focusedWs: boolean;
  focusedTermId: string | null;
  onFocusWorkspace: (wsId: string) => void;
  onJumpTerm: (termId: string) => void;
  onCloseTerm: (t: TerminalInfo) => void;
  onRemoveWorkspace: (ws: WorkspaceInfo) => void;
  onDiff: (ws: WorkspaceInfo) => void;
}) {
  const name = ws.label ?? ws.branch ?? basename(ws.path) ?? ws.workspace_id;
  const remove = async () => {
    const ok = await confirmClose(
      "Remove Workspace",
      `Remove workspace “${name}”? Its ${panes.length} pane(s) will be killed and the worktree checkout deleted.`,
    );
    if (ok) onRemoveWorkspace(ws);
  };
  return (
    <div className="side-tree-group">
      <div className={`side-ws-head ${focusedWs ? "focused" : ""}`}>
        <button
          type="button"
          className="side-ws-label side-primary-label"
          onClick={() => onFocusWorkspace(ws.workspace_id)}
          title={ws.path}
        >
          <HugeiconsIcon
            icon={GitBranchIcon}
            size={12}
            strokeWidth={1.5}
            className="side-ico"
          />
          <span className="side-ws-name">{name}</span>
          {ws.is_main && <span className="side-pill">main</span>}
          {panes.length > 0 && (
            <span className="side-tree-count">{panes.length}</span>
          )}
        </button>
        {!ws.is_main && (
          <button
            type="button"
            className="side-close"
            title={`diff ${ws.path}`}
            onClick={() => onDiff(ws)}
          >
            <HugeiconsIcon icon={GitCompareIcon} size={11} strokeWidth={1.5} />
          </button>
        )}
        {!ws.is_main && (
          <button
            type="button"
            className="side-close"
            title="remove workspace (worktree)"
            onClick={remove}
          >
            ✕
          </button>
        )}
      </div>
      {panes.length > 0 && (
        <div className="side-tree-children">
          {panes.map((t) => (
            <PaneRow
              key={t.term_id}
              t={t}
              focused={t.term_id === focusedTermId}
              onFocus={() => onJumpTerm(t.term_id)}
              onClose={() => onCloseTerm(t)}
            />
          ))}
        </div>
      )}
    </div>
  );
}

export function Sidebar({
  snap,
  focusedWorkspaceId,
  focusedTermId,
  onFocusProject,
  onFocusWorkspace,
  onJumpTerm,
  onNewProject,
  onNewGroup,
  onCloseProject,
  onRenameProject,
  onRenameGroup,
  onRemoveGroup,
  onAssignProject,
  onCloseTerm,
  onRemoveWorkspace,
  onDiff,
  onOrchestrate,
}: {
  snap: Snapshot | null;
  focusedWorkspaceId: string | null;
  focusedTermId: string | null;
  onFocusProject: (id: string) => void;
  onFocusWorkspace: (wsId: string) => void;
  onJumpTerm: (termId: string) => void;
  onNewProject: () => void;
  /** creates a group — resolves to its id so the caller can enter rename */
  onNewGroup: () => Promise<string | null>;
  onCloseProject: (id: string) => void;
  onRenameProject: (id: string, label: string) => void;
  onRenameGroup: (id: string, label: string) => void;
  onRemoveGroup: (id: string) => void;
  /** groupId null = move back to ungrouped */
  onAssignProject: (projectId: string, groupId: string | null) => void;
  onCloseTerm: (t: TerminalInfo) => void;
  onRemoveWorkspace: (ws: WorkspaceInfo) => void;
  onDiff: (ws: WorkspaceInfo) => void;
  onOrchestrate: (projectId: string) => void;
}) {
  const [renaming, setRenaming] = useState<string | null>(null);
  const [renamingGroup, setRenamingGroup] = useState<string | null>(null);
  const [collapsed, setCollapsed] =
    useState<ReadonlySet<string>>(loadCollapsed);
  const { width, sideRef, onResizeDown, resetWidth } = useSidebarWidth();
  const [menu, setMenu] = useState<{
    kind: "project" | "group";
    key: string;
    x: number;
    y: number;
  } | null>(null);
  const projects = snap?.projects ?? [];
  const groups = snap?.groups ?? [];
  const termsById = new Map((snap?.terminals ?? []).map((t) => [t.term_id, t]));
  const wsById = new Map(
    (snap?.workspaces ?? []).map((w) => [w.workspace_id, w]),
  );
  const tabById = new Map((snap?.tabs ?? []).map((t) => [t.tab_id, t]));
  const projById = new Map(projects.map((p) => [p.project_id, p]));
  const groupedIds = new Set(groups.flatMap((g) => g.projects));
  const ungrouped = projects.filter((p) => !groupedIds.has(p.project_id));

  /** a workspace's panes in display order — tab order, then pane order */
  const wsPanes = (ws: WorkspaceInfo): TerminalInfo[] =>
    ws.tabs
      .flatMap((tid) => tabById.get(tid)?.panes ?? [])
      .map((pid) => termsById.get(pid))
      .filter((t): t is TerminalInfo => Boolean(t));

  const toggleGroup = (id: string) =>
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      localStorage.setItem(COLLAPSED_KEY, JSON.stringify([...next]));
      return next;
    });

  const renderProject = (p: ProjectInfo) => {
    const workspaces = p.workspaces
      .map((id) => wsById.get(id))
      .filter((w): w is WorkspaceInfo => Boolean(w));
    const name = p.label ?? basename(p.repo_root) ?? p.project_id;
    const projFocused = workspaces.some(
      (w) => w.workspace_id === focusedWorkspaceId,
    );

    return (
      <div
        key={p.project_id}
        className={`side-proj ${projFocused ? "focused" : ""}`}
      >
        <div className="side-proj-head">
          {renaming === p.project_id ? (
            <RenameInput
              initial={p.label ?? ""}
              onCommit={(l) => {
                if (l) onRenameProject(p.project_id, l);
                setRenaming(null);
              }}
              onCancel={() => setRenaming(null)}
            />
          ) : (
            <button
              type="button"
              className="side-proj-label"
              onClick={() => onFocusProject(p.project_id)}
              onDoubleClick={() => setRenaming(p.project_id)}
              title={`${p.repo_root} (double-click to rename)`}
            >
              <HugeiconsIcon
                icon={Folder01Icon}
                size={13}
                strokeWidth={1.5}
                className="side-proj-ico"
              />
              <span>{name}</span>
            </button>
          )}
          <button
            type="button"
            className="side-close"
            title="start orchestrator agent"
            onClick={() => onOrchestrate(p.project_id)}
          >
            <HugeiconsIcon icon={SparklesIcon} size={12} strokeWidth={1.5} />
          </button>
          <button
            type="button"
            className="side-close"
            title="project settings"
            onClick={(e) => {
              e.stopPropagation();
              const r = e.currentTarget.getBoundingClientRect();
              setMenu({
                kind: "project",
                key: p.project_id,
                x: r.left,
                y: r.bottom + 4,
              });
            }}
          >
            <HugeiconsIcon
              icon={MoreHorizontalIcon}
              size={12}
              strokeWidth={1.5}
            />
          </button>
        </div>
        {workspaces.length > 0 && (
          <div className="side-proj-children">
            {workspaces.map((ws) => (
              <WorkspaceRow
                key={ws.workspace_id}
                ws={ws}
                panes={wsPanes(ws)}
                focusedWs={ws.workspace_id === focusedWorkspaceId}
                focusedTermId={focusedTermId}
                onFocusWorkspace={onFocusWorkspace}
                onJumpTerm={onJumpTerm}
                onCloseTerm={onCloseTerm}
                onRemoveWorkspace={onRemoveWorkspace}
                onDiff={onDiff}
              />
            ))}
          </div>
        )}
      </div>
    );
  };

  const renderGroup = (g: GroupInfo) => {
    const members = g.projects
      .map((id) => projById.get(id))
      .filter((p): p is ProjectInfo => Boolean(p));
    const isCollapsed = collapsed.has(g.group_id);
    const label = g.label ?? "Group";
    return (
      <div key={g.group_id} className="side-group">
        <div className="side-group-head">
          {renamingGroup === g.group_id ? (
            <RenameInput
              initial={g.label ?? ""}
              onCommit={(l) => {
                if (l) onRenameGroup(g.group_id, l);
                setRenamingGroup(null);
              }}
              onCancel={() => setRenamingGroup(null)}
            />
          ) : (
            <button
              type="button"
              className="side-group-label"
              onClick={() => toggleGroup(g.group_id)}
              onDoubleClick={() => setRenamingGroup(g.group_id)}
              title={`${g.projects.length} project(s) — double-click to rename`}
            >
              <HugeiconsIcon
                icon={isCollapsed ? ArrowRight01Icon : ArrowDown01Icon}
                size={11}
                strokeWidth={2}
                className="side-caret"
              />
              <HugeiconsIcon
                icon={GroupLayersIcon}
                size={12}
                strokeWidth={1.5}
                className="side-ico"
              />
              <span className="side-group-name">{label}</span>
              <span className="side-tree-count">{members.length}</span>
            </button>
          )}
          <button
            type="button"
            className="side-close"
            title="group options"
            onClick={(e) => {
              e.stopPropagation();
              const r = e.currentTarget.getBoundingClientRect();
              setMenu({
                kind: "group",
                key: g.group_id,
                x: r.left,
                y: r.bottom + 4,
              });
            }}
          >
            <HugeiconsIcon
              icon={MoreHorizontalIcon}
              size={12}
              strokeWidth={1.5}
            />
          </button>
        </div>
        {!isCollapsed && (
          <div className="side-group-children">
            {members.map((p) => renderProject(p))}
          </div>
        )}
      </div>
    );
  };

  const rows: React.ReactNode[] = [];
  for (const g of groups) rows.push(renderGroup(g));
  for (const p of ungrouped) rows.push(renderProject(p));

  useEffect(() => {
    if (!menu) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setMenu(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [menu]);

  const removeProject = async () => {
    const m = menu;
    setMenu(null);
    if (!m || m.kind !== "project") return;
    const p = projects.find((x) => x.project_id === m.key);
    if (!p) return;
    const name = p.label ?? basename(p.repo_root) ?? p.project_id;
    const n = p.workspaces.length;
    const ok = await ask(`Close project “${name}” and its ${n} workspace(s)?`, {
      title: "Remove Project",
      kind: "warning",
      okLabel: "Remove",
      cancelLabel: "Cancel",
    }).catch(() => false);
    if (ok) onCloseProject(p.project_id);
  };

  const removeGroup = async () => {
    const m = menu;
    setMenu(null);
    if (!m || m.kind !== "group") return;
    const g = groups.find((x) => x.group_id === m.key);
    if (!g) return;
    const name = g.label ?? "Group";
    const ok = await ask(
      `Remove group “${name}”? Its ${g.projects.length} project(s) become ungrouped — nothing is closed.`,
      {
        title: "Remove Group",
        kind: "warning",
        okLabel: "Remove",
        cancelLabel: "Cancel",
      },
    ).catch(() => false);
    if (ok) onRemoveGroup(g.group_id);
  };

  const menuProject =
    menu?.kind === "project"
      ? projects.find((x) => x.project_id === menu.key)
      : undefined;
  const menuGroup =
    menu?.kind === "group"
      ? groups.find((x) => x.group_id === menu.key)
      : undefined;

  return (
    <div ref={sideRef} className="sidebar" style={{ width }}>
      <div className="side-head">
        <span className="side-head-label">PROJECTS</span>
        <button
          type="button"
          className="side-head-btn"
          title="new group"
          onClick={async () => {
            const id = await onNewGroup();
            if (id) setRenamingGroup(id);
          }}
        >
          <HugeiconsIcon icon={FolderPlusIcon} size={22} strokeWidth={1.5} />
        </button>
        <button
          type="button"
          className="side-head-btn"
          title="import project (⇧⌘N)"
          onClick={onNewProject}
        >
          <HugeiconsIcon icon={PlusIcon} size={22} strokeWidth={1.5} />
        </button>
      </div>
      <div className="side-scroll">{rows}</div>
      <div
        className="divider x side-resize"
        onMouseDown={onResizeDown}
        onDoubleClick={resetWidth}
        title="drag to resize · double-click to reset"
      />
      {menu && (
        <div className="proj-menu-overlay" onMouseDown={() => setMenu(null)}>
          <div
            className="proj-menu"
            style={{
              left: Math.min(menu.x, window.innerWidth - 190),
              top: menu.y,
            }}
            onMouseDown={(e) => e.stopPropagation()}
          >
            {menu.kind === "project" && menuProject && (
              <>
                <div className="proj-menu-label">Move to group</div>
                <button
                  type="button"
                  className="proj-menu-item"
                  onClick={() => {
                    if (menuProject.group_id) {
                      onAssignProject(menuProject.project_id, null);
                    }
                    setMenu(null);
                  }}
                >
                  <span className="proj-menu-check">
                    {menuProject.group_id ? "" : "✓"}
                  </span>
                  No group
                </button>
                {groups.map((g) => (
                  <button
                    key={g.group_id}
                    type="button"
                    className="proj-menu-item"
                    onClick={() => {
                      if (menuProject.group_id !== g.group_id) {
                        onAssignProject(menuProject.project_id, g.group_id);
                      }
                      setMenu(null);
                    }}
                  >
                    <span className="proj-menu-check">
                      {menuProject.group_id === g.group_id ? "✓" : ""}
                    </span>
                    {g.label ?? "Group"}
                  </button>
                ))}
                <button
                  type="button"
                  className="proj-menu-item"
                  onClick={async () => {
                    const id = await onNewGroup();
                    if (id) {
                      onAssignProject(menuProject.project_id, id);
                      setRenamingGroup(id);
                    }
                    setMenu(null);
                  }}
                >
                  <span className="proj-menu-check" />
                  New group…
                </button>
                <div className="proj-menu-sep" />
                <button
                  type="button"
                  className="proj-menu-item danger"
                  onClick={removeProject}
                >
                  <HugeiconsIcon
                    icon={Delete01Icon}
                    size={13}
                    strokeWidth={1.5}
                  />
                  Remove Project
                </button>
              </>
            )}
            {menu.kind === "group" && menuGroup && (
              <>
                <button
                  type="button"
                  className="proj-menu-item"
                  onClick={() => {
                    setRenamingGroup(menuGroup.group_id);
                    setMenu(null);
                  }}
                >
                  <HugeiconsIcon
                    icon={PencilEdit01Icon}
                    size={13}
                    strokeWidth={1.5}
                  />
                  Rename Group
                </button>
                <div className="proj-menu-sep" />
                <button
                  type="button"
                  className="proj-menu-item danger"
                  onClick={removeGroup}
                >
                  <HugeiconsIcon
                    icon={Delete01Icon}
                    size={13}
                    strokeWidth={1.5}
                  />
                  Remove Group
                </button>
              </>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
