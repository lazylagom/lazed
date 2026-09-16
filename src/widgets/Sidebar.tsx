import {
  ArrowDown01Icon,
  ArrowRight01Icon,
  BotIcon,
  Delete02Icon,
  FolderAddIcon,
  FolderGitIcon,
  GitBranchIcon,
  GroupItemsIcon,
  MoreHorizontalIcon,
  PlusIcon,
  TerminalIcon,
  TowerControlIcon,
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
} from "../shared/lazed";
import { SIDEBAR_RESET_EVENT } from "../shared/settings";

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

const SIDE_MIN = 180;
const SIDE_MAX = 560;
const SIDE_DEFAULT = 288;
const SIDE_KEY = "sidebar-width";
const COLLAPSED_KEY = "sidebar-collapsed-groups";

function loadSideWidth() {
  const v = Number(localStorage.getItem(SIDE_KEY));
  if (!Number.isFinite(v) || v <= 0) return SIDE_DEFAULT;
  return Math.min(SIDE_MAX, Math.max(SIDE_MIN, v));
}

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

/** A terminal child row — a worktree or plain terminal inside a project. */
function TermRow({
  t,
  num,
  focused,
  onFocus,
  onClose,
  onDiff,
}: {
  t: TerminalInfo;
  num?: number;
  focused: boolean;
  onFocus: () => void;
  onClose: () => void;
  onDiff?: () => void;
}) {
  const name =
    t.label ?? t.branch ?? t.agent_kind ?? basename(t.cwd) ?? t.term_id;
  const close = async () => {
    const ok = await confirmClose(
      t.kind === "worktree" ? "Remove Worktree" : "Close Terminal",
      t.kind === "worktree"
        ? `Remove worktree “${name}”? Its terminal will be killed and the checkout deleted.`
        : `Close terminal “${name}”?`,
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
          icon={
            t.agent_kind
              ? BotIcon
              : t.kind === "worktree"
                ? GitBranchIcon
                : TerminalIcon
          }
          size={12}
          strokeWidth={1.5}
          className="side-ico"
        />
        {num !== undefined && (
          <span className="side-ws-num" title={`terminal ${num}`}>
            {num}
          </span>
        )}
        <span className={`side-ws-name ${statusClass(t.agent_status)}`}>
          {name}
        </span>
      </button>
      {t.kind === "worktree" && onDiff && (
        <button
          type="button"
          className="side-close"
          title={`diff ${t.cwd}`}
          onClick={onDiff}
        >
          <HugeiconsIcon icon={GitBranchIcon} size={11} strokeWidth={1.5} />
        </button>
      )}
      <button
        type="button"
        className="side-close"
        title={t.kind === "worktree" ? "remove worktree" : "close terminal"}
        onClick={close}
      >
        ✕
      </button>
    </div>
  );
}

export function Sidebar({
  snap,
  focusedTermId,
  onFocusProject,
  onJumpTerm,
  onNewProject,
  onNewGroup,
  onCloseProject,
  onRenameProject,
  onRenameGroup,
  onRemoveGroup,
  onAssignProject,
  onCloseTerm,
  onDiff,
  onOrchestrate,
  rollup,
}: {
  snap: Snapshot | null;
  focusedTermId: string | null;
  onFocusProject: (id: string) => void;
  onJumpTerm: (termId: string, projectId: string) => void;
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
  onDiff: (t: TerminalInfo) => void;
  onOrchestrate: (projectId: string) => void;
  rollup: (s: (AgentStatus | undefined)[]) => AgentStatus;
}) {
  const [renaming, setRenaming] = useState<string | null>(null);
  const [renamingGroup, setRenamingGroup] = useState<string | null>(null);
  const [collapsed, setCollapsed] =
    useState<ReadonlySet<string>>(loadCollapsed);
  const [width, setWidth] = useState(loadSideWidth);
  const [menu, setMenu] = useState<{
    kind: "project" | "group";
    key: string;
    x: number;
    y: number;
  } | null>(null);
  const sideRef = useRef<HTMLDivElement>(null);
  const projects = snap?.projects ?? [];
  const groups = snap?.groups ?? [];
  const termsById = new Map((snap?.terminals ?? []).map((t) => [t.term_id, t]));
  const projById = new Map(projects.map((p) => [p.project_id, p]));
  const groupedIds = new Set(groups.flatMap((g) => g.projects));
  const ungrouped = projects.filter((p) => !groupedIds.has(p.project_id));

  const toggleGroup = (id: string) =>
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      localStorage.setItem(COLLAPSED_KEY, JSON.stringify([...next]));
      return next;
    });

  const renderProject = (p: ProjectInfo, idx: number) => {
    const terms = p.terminals
      .map((id) => termsById.get(id))
      .filter((t): t is TerminalInfo => Boolean(t));
    const root = terms.find((t) => t.kind !== "worktree") ?? terms[0];
    const worktrees = terms.filter((t) => t.kind === "worktree");
    const plains = terms.filter((t) => t.kind !== "worktree");
    const name = p.label ?? basename(p.repo_root) ?? p.project_id;
    const projStatus = rollup(terms.map((t) => t.agent_status));

    return (
      <div
        key={p.project_id}
        className={`side-proj ${p.focused ? "focused" : ""}`}
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
              onClick={() =>
                root
                  ? onJumpTerm(root.term_id, p.project_id)
                  : onFocusProject(p.project_id)
              }
              onDoubleClick={() => setRenaming(p.project_id)}
              title={`${p.repo_root} — root terminal (double-click to rename)`}
            >
              <HugeiconsIcon
                icon={FolderGitIcon}
                size={13}
                strokeWidth={1.5}
                className="side-proj-ico"
              />
              <span className="side-ws-num" title={`project ${idx + 1}`}>
                {idx + 1}
              </span>
              <span className={statusClass(projStatus)}>{name}</span>
            </button>
          )}
          <button
            type="button"
            className="side-close"
            title="start orchestrator agent"
            onClick={() => onOrchestrate(p.project_id)}
          >
            <HugeiconsIcon
              icon={TowerControlIcon}
              size={12}
              strokeWidth={1.5}
            />
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
        {terms.length > 0 && (
          <div className="side-proj-children">
            {plains.length > 0 && (
              <div className="side-tree-group">
                <button
                  type="button"
                  className="side-tree-label side-primary-label"
                  onClick={() => root && onJumpTerm(root.term_id, p.project_id)}
                  title={root?.cwd}
                >
                  <span
                    className={`dot ${rollup(plains.map((t) => t.agent_status))}`}
                  />
                  <span className="side-primary-name">
                    {root?.branch ?? basename(p.repo_root)}
                  </span>
                  <span className="side-pill">primary</span>
                  <span className="side-tree-count">{plains.length}</span>
                </button>
                <div className="side-tree-children">
                  {plains.map((t) => (
                    <TermRow
                      key={t.term_id}
                      t={t}
                      focused={t.term_id === focusedTermId}
                      onFocus={() => onJumpTerm(t.term_id, p.project_id)}
                      onClose={() => onCloseTerm(t)}
                    />
                  ))}
                </div>
              </div>
            )}
            {worktrees.map((t) => (
              <TermRow
                key={t.term_id}
                t={t}
                focused={t.term_id === focusedTermId}
                onFocus={() => onJumpTerm(t.term_id, p.project_id)}
                onClose={() => onCloseTerm(t)}
                onDiff={() => onDiff(t)}
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
    const gStatus = rollup(
      members.flatMap((p) =>
        p.terminals.map((id) => termsById.get(id)?.agent_status),
      ),
    );
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
                icon={GroupItemsIcon}
                size={12}
                strokeWidth={1.5}
                className="side-ico"
              />
              <span className={`side-group-name ${statusClass(gStatus)}`}>
                {label}
              </span>
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
            {members.map((p) => renderProject(p, projects.indexOf(p)))}
          </div>
        )}
      </div>
    );
  };

  const rows: React.ReactNode[] = [];
  for (const g of groups) rows.push(renderGroup(g));
  for (const p of ungrouped) rows.push(renderProject(p, projects.indexOf(p)));

  const onResizeDown = (e: React.MouseEvent) => {
    e.preventDefault();
    const startX = e.clientX;
    const startW = sideRef.current?.getBoundingClientRect().width ?? width;
    const clamp = (v: number) => Math.min(SIDE_MAX, Math.max(SIDE_MIN, v));
    const onMove = (ev: MouseEvent) =>
      setWidth(clamp(startW + ev.clientX - startX));
    const onUp = (ev: MouseEvent) => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      const w = Math.round(clamp(startW + ev.clientX - startX));
      localStorage.setItem(SIDE_KEY, String(w));
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  };

  const resetWidth = () => {
    setWidth(SIDE_DEFAULT);
    localStorage.setItem(SIDE_KEY, String(SIDE_DEFAULT));
  };

  useEffect(() => {
    const onReset = () => {
      setWidth(SIDE_DEFAULT);
      localStorage.setItem(SIDE_KEY, String(SIDE_DEFAULT));
    };
    window.addEventListener(SIDEBAR_RESET_EVENT, onReset);
    return () => window.removeEventListener(SIDEBAR_RESET_EVENT, onReset);
  }, []);

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
    const ok = await ask(
      `Close project “${name}” and its ${p.terminals.length} terminal(s)?`,
      {
        title: "Remove Project",
        kind: "warning",
        okLabel: "Remove",
        cancelLabel: "Cancel",
      },
    ).catch(() => false);
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
        <span className="side-head-label">Projects</span>
        <button
          type="button"
          className="side-head-btn"
          title="new group"
          onClick={async () => {
            const id = await onNewGroup();
            if (id) setRenamingGroup(id);
          }}
        >
          <HugeiconsIcon icon={FolderAddIcon} size={13} strokeWidth={1.5} />
        </button>
        <button
          type="button"
          className="side-head-btn"
          title="import project (⇧⌘N)"
          onClick={onNewProject}
        >
          <HugeiconsIcon icon={PlusIcon} size={13} strokeWidth={1.5} />
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
                    icon={Delete02Icon}
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
                    icon={MoreHorizontalIcon}
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
                    icon={Delete02Icon}
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
