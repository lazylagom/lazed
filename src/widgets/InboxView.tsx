import {
  ArrowUpRight01Icon,
  CheckmarkCircle02Icon,
  Clock01Icon,
  InboxIcon,
  SentIcon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { useEffect, useRef, useState } from "react";
import { type InboxItem, inbox, openUrl } from "../shared/inbox";
import {
  AGENT_KINDS,
  type ProjectInfo,
  type TerminalInfo,
  lazed,
} from "../shared/lazed";
import { useSidebarWidth } from "./sidebar-width";

function relTime(at: number): string {
  const s = Math.max(0, Math.floor(Date.now() / 1000) - at);
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  return `${Math.floor(s / 86400)}d`;
}

function snoozeLabel(until?: number): string {
  if (!until) return "";
  const s = until - Math.floor(Date.now() / 1000);
  if (s < 3600) return `${Math.max(1, Math.round(s / 60))}m`;
  if (s < 86400) return `${Math.round(s / 3600)}h`;
  return `${Math.round(s / 86400)}d`;
}

/** Prompt text handed to an agent or pane for a delegated item. */
function delegateText(item: InboxItem): string {
  return [item.title, item.url, item.body]
    .filter(Boolean)
    .join("\n")
    .slice(0, 8000);
}

type Menu =
  | { kind: "snooze"; item: InboxItem; x: number; y: number }
  | { kind: "delegate"; item: InboxItem; x: number; y: number };

const SNOOZES: { label: string; at: () => number }[] = [
  {
    label: "in 1 hour",
    at: () => Math.floor(Date.now() / 1000) + 3600,
  },
  {
    label: "tomorrow 9:00",
    at: () => {
      const d = new Date();
      d.setDate(d.getDate() + 1);
      d.setHours(9, 0, 0, 0);
      return Math.floor(d.getTime() / 1000);
    },
  },
  {
    label: "in 3 days",
    at: () => Math.floor(Date.now() / 1000) + 3 * 86400,
  },
];

/** The GTD inbox — every captured item lands here for triage. Sources:
 * quick-add below, `lazed inbox add`, and automations with an inbox action
 * (Jira, Slack, anything that prints one item per line). */
export function InboxView({
  items,
  error,
  projects,
  focusedProjectId,
  focusedTerm,
  onChanged,
  onFlash,
  onError,
}: {
  items: InboxItem[];
  /** e.g. "unknown method" when the running daemon predates inbox.v1 */
  error?: string | null;
  projects: ProjectInfo[];
  focusedProjectId?: string;
  focusedTerm?: TerminalInfo;
  onChanged: () => void;
  onFlash: (msg: string) => void;
  onError: (msg: string) => void;
}) {
  const { width, sideRef, onResizeDown, resetWidth } = useSidebarWidth();
  const [draft, setDraft] = useState("");
  const [menu, setMenu] = useState<Menu | null>(null);
  const [showSnoozed, setShowSnoozed] = useState(false);
  const [delegateProject, setDelegateProject] = useState("");
  const [delegateKind, setDelegateKind] = useState("claude");
  const [busy, setBusy] = useState(false);
  const addRef = useRef<HTMLInputElement>(null);

  const open = items.filter((i) => i.status === "open");
  const snoozed = items.filter((i) => i.status === "snoozed");

  useEffect(() => {
    if (!menu) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setMenu(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [menu]);

  const run = (p: Promise<unknown>, ok?: string) =>
    p
      .then(() => {
        if (ok) onFlash(ok);
        onChanged();
      })
      .catch((e) => onError(String(e)));

  const capture = () => {
    const title = draft.trim();
    if (!title) return;
    setDraft("");
    run(inbox.add(title));
  };

  const openMenu = (
    e: React.MouseEvent,
    item: InboxItem,
    kind: Menu["kind"],
  ) => {
    const r = e.currentTarget.getBoundingClientRect();
    if (kind === "delegate" && !delegateProject) {
      setDelegateProject(focusedProjectId ?? projects[0]?.project_id ?? "");
    }
    setMenu({ kind, item, x: r.right + 6, y: r.top });
  };

  const delegateSpawn = async (item: InboxItem) => {
    const project = projects.find((p) => p.project_id === delegateProject);
    if (!project) {
      onError("delegate: pick a project");
      return;
    }
    setBusy(true);
    try {
      const res = await lazed.taskStart({
        cwd: project.repo_root,
        kind: delegateKind,
        text: delegateText(item),
        branch: "",
        request_id: `inbox-${item.id}`,
      });
      if (res.error) {
        onError(`task ${res.task_id}: ${res.error}`);
      } else {
        onFlash(`delegated — task ${res.task_id} (${delegateKind})`);
        await inbox.update(item.id, { status: "done" });
      }
      setMenu(null);
      onChanged();
    } catch (e) {
      onError(
        `${String(e)}; request_id inbox-${item.id} — query before retrying`,
      );
    } finally {
      setBusy(false);
    }
  };

  const delegateToPane = async (item: InboxItem) => {
    if (!focusedTerm) return;
    setBusy(true);
    try {
      const text = delegateText(item);
      if (focusedTerm.agent_kind) {
        await lazed.agentPrompt(focusedTerm.term_id, text);
      } else {
        await lazed.termSend(focusedTerm.term_id, `${text}\n`);
      }
      onFlash(`sent to ${focusedTerm.label ?? focusedTerm.term_id}`);
      await inbox.update(item.id, { status: "done" });
      setMenu(null);
      onChanged();
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const row = (item: InboxItem) => (
    <div key={item.id} className="ibx-row">
      <div className="ibx-main">
        <div className="ibx-title" title={item.body ?? item.title}>
          {item.title}
        </div>
        <div className="ibx-meta">
          <span className="ibx-source">{item.source}</span>
          <span>{relTime(item.at)}</span>
          {item.status === "snoozed" && (
            <span className="ibx-until">
              ⏰ {snoozeLabel(item.snooze_until)}
            </span>
          )}
        </div>
      </div>
      <div className="ibx-actions">
        {item.url && (
          <button
            type="button"
            className="ibx-btn"
            title={`open ${item.url}`}
            onClick={() =>
              openUrl(item.url ?? "").catch((e) => onError(String(e)))
            }
          >
            <HugeiconsIcon
              icon={ArrowUpRight01Icon}
              size={12}
              strokeWidth={1.5}
            />
          </button>
        )}
        <button
          type="button"
          className="ibx-btn"
          title="snooze"
          onClick={(e) => openMenu(e, item, "snooze")}
        >
          <HugeiconsIcon icon={Clock01Icon} size={12} strokeWidth={1.5} />
        </button>
        <button
          type="button"
          className="ibx-btn"
          title="delegate to agent / pane"
          onClick={(e) => openMenu(e, item, "delegate")}
        >
          <HugeiconsIcon icon={SentIcon} size={12} strokeWidth={1.5} />
        </button>
        <button
          type="button"
          className="ibx-btn ibx-done"
          title="mark done"
          onClick={() => run(inbox.update(item.id, { status: "done" }))}
        >
          <HugeiconsIcon
            icon={CheckmarkCircle02Icon}
            size={12}
            strokeWidth={1.5}
          />
        </button>
      </div>
    </div>
  );

  return (
    <div ref={sideRef} className="sidebar" style={{ width }}>
      <div className="side-head">
        <span className="side-head-label">
          Inbox{open.length > 0 ? ` · ${open.length}` : ""}
        </span>
        <button
          type="button"
          className="side-head-btn"
          title="capture"
          onClick={() => addRef.current?.focus()}
        >
          <HugeiconsIcon icon={InboxIcon} size={17} strokeWidth={1.5} />
        </button>
      </div>
      <div className="ibx-add">
        <input
          ref={addRef}
          className="ibx-add-input"
          placeholder="capture… (⏎ adds)"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") capture();
          }}
        />
      </div>
      <div className="side-scroll">
        {error ? (
          <div className="ibx-empty">
            inbox unavailable: {error}
            <br />
            <span className="ibx-empty-sub">
              an older daemon may be running — restart it after rebuilding
            </span>
          </div>
        ) : (
          open.length === 0 &&
          snoozed.length === 0 && (
            <div className="ibx-empty">
              inbox zero — captured items land here
              <br />
              <span className="ibx-empty-sub">
                lazed inbox add · automation → inbox action
              </span>
            </div>
          )
        )}
        {open.map(row)}
        {snoozed.length > 0 && (
          <button
            type="button"
            className="ibx-snoozed-toggle"
            onClick={() => setShowSnoozed((v) => !v)}
          >
            {showSnoozed ? "▾" : "▸"} snoozed ({snoozed.length})
          </button>
        )}
        {showSnoozed && snoozed.map(row)}
      </div>
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
              left: Math.min(menu.x, window.innerWidth - 240),
              top: menu.y,
            }}
            onMouseDown={(e) => e.stopPropagation()}
          >
            {menu.kind === "snooze" &&
              SNOOZES.map((s) => (
                <button
                  key={s.label}
                  type="button"
                  className="proj-menu-item"
                  onClick={() => {
                    setMenu(null);
                    run(
                      inbox.update(menu.item.id, { snooze_until: s.at() }),
                      `snoozed — ${s.label}`,
                    );
                  }}
                >
                  {s.label}
                </button>
              ))}
            {menu.kind === "delegate" && (
              <>
                <div className="proj-menu-label">delegate</div>
                <div className="ibx-delegate">
                  <select
                    className="modal-input"
                    value={delegateProject}
                    onChange={(e) => setDelegateProject(e.target.value)}
                  >
                    {projects.length === 0 && (
                      <option value="">no project</option>
                    )}
                    {projects.map((p) => (
                      <option key={p.project_id} value={p.project_id}>
                        {p.label ?? p.project_id}
                      </option>
                    ))}
                  </select>
                  <select
                    className="modal-input"
                    value={delegateKind}
                    onChange={(e) => setDelegateKind(e.target.value)}
                  >
                    {AGENT_KINDS.map((k) => (
                      <option key={k} value={k}>
                        {k}
                      </option>
                    ))}
                  </select>
                </div>
                <button
                  type="button"
                  className="proj-menu-item"
                  disabled={busy || !delegateProject}
                  onClick={() => delegateSpawn(menu.item)}
                >
                  <span className="proj-menu-check">▸</span>
                  spawn task — worktree + agent
                </button>
                <button
                  type="button"
                  className="proj-menu-item"
                  disabled={busy || !focusedTerm}
                  title={
                    focusedTerm
                      ? `send to ${focusedTerm.label ?? focusedTerm.term_id}`
                      : "no focused pane"
                  }
                  onClick={() => delegateToPane(menu.item)}
                >
                  <span className="proj-menu-check">→</span>
                  send to focused pane
                </button>
              </>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
