import {
  ArrowUpRight01Icon,
  Calendar01Icon,
  CheckListIcon,
  CheckmarkCircle02Icon,
  Clock01Icon,
  DiscordIcon,
  FigmaIcon,
  GithubIcon,
  GitlabIcon,
  GoogleIcon,
  InboxIcon,
  Mail01Icon,
  NotionIcon,
  QuillWrite01Icon,
  SentIcon,
  SlackIcon,
  Task01Icon,
  TrelloIcon,
  WorkflowIcon,
  ZoomIcon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon, type IconSvgElement } from "@hugeicons/react";
import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import {
  type InboxItem,
  inbox,
  jiraIssueOf,
  openUrl,
  providerOf,
} from "../shared/inbox";
import {
  AGENT_KINDS,
  type PaneInfo,
  type ProjectInfo,
  lazed,
} from "../shared/lazed";

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

/** Per-provider icon + label for section headers — Akiflow-style. Unknown
 * slugs (custom automation sources) fall back to the workflow glyph and the
 * source name itself. */
const PROVIDER_META: Record<string, { label: string; icon: IconSvgElement }> = {
  manual: { label: "Captured", icon: QuillWrite01Icon },
  jira: { label: "Jira", icon: Task01Icon },
  slack: { label: "Slack", icon: SlackIcon },
  gmail: { label: "Gmail", icon: Mail01Icon },
  mail: { label: "Mail", icon: Mail01Icon },
  github: { label: "GitHub", icon: GithubIcon },
  gitlab: { label: "GitLab", icon: GitlabIcon },
  notion: { label: "Notion", icon: NotionIcon },
  trello: { label: "Trello", icon: TrelloIcon },
  figma: { label: "Figma", icon: FigmaIcon },
  discord: { label: "Discord", icon: DiscordIcon },
  google: { label: "Google", icon: GoogleIcon },
  calendar: { label: "Calendar", icon: Calendar01Icon },
  zoom: { label: "Zoom", icon: ZoomIcon },
  linear: { label: "Linear", icon: WorkflowIcon },
  asana: { label: "Asana", icon: CheckListIcon },
  todoist: { label: "Todoist", icon: CheckListIcon },
};

function providerMeta(key: string) {
  return (
    PROVIDER_META[key] ?? {
      label: key,
      icon: WorkflowIcon,
    }
  );
}

/** Cluster a jira group's rows by issue — assignments ("CS-1") and each
 * mention ("CS-1#commentId") collapse under one head per issue. Rows with
 * no detectable key stay as standalone rows, in place. */
function issueClusters(items: InboxItem[]) {
  const out: { issue: string | null; items: InboxItem[] }[] = [];
  const byIssue = new Map<string, { issue: string; items: InboxItem[] }>();
  for (const item of items) {
    const issue = jiraIssueOf(item);
    if (!issue) {
      out.push({ issue: null, items: [item] });
      continue;
    }
    let c = byIssue.get(issue);
    if (!c) {
      c = { issue, items: [] };
      byIssue.set(issue, c);
      out.push(c);
    }
    c.items.push(item);
  }
  return out;
}

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

/** The GTD inbox — every captured item lands here for triage, grouped into
 * collapsible provider sections (Jira, Slack, Gmail… Akiflow-style). Sources:
 * quick-add below, `lazed inbox add`, and automations with an inbox action
 * (a preset supplies the provider; a custom poller can emit a JSON
 * `provider` field, and its name groups anything else). */
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
  focusedTerm?: PaneInfo;
  onChanged: () => void;
  onFlash: (msg: string) => void;
  onError: (msg: string) => void;
}) {
  const [draft, setDraft] = useState("");
  const [menu, setMenu] = useState<Menu | null>(null);
  const [showSnoozed, setShowSnoozed] = useState(false);
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const [delegateProject, setDelegateProject] = useState("");
  const [delegateKind, setDelegateKind] = useState("claude");
  const [busy, setBusy] = useState(false);
  const addRef = useRef<HTMLInputElement>(null);

  const open = items.filter((i) => i.status === "open");
  const snoozed = items.filter((i) => i.status === "snoozed");

  // provider sections, ordered by each group's newest item (items arrive
  // newest-first from the daemon — first occurrence wins). Done items stay
  // in place, marked; snoozed items live in the collapsed section below.
  const groups = useMemo(() => {
    const byKey = new Map<string, InboxItem[]>();
    for (const item of items) {
      if (item.status === "snoozed") continue;
      const key = providerOf(item);
      const bucket = byKey.get(key);
      if (bucket) bucket.push(item);
      else byKey.set(key, [item]);
    }
    return [...byKey.entries()].map(([key, items]) => ({
      key,
      items,
      ...providerMeta(key),
    }));
  }, [items]);

  const toggleGroup = (key: string) =>
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });

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
      if (res.error || res.receipt?.accepted !== true) {
        onError(
          `task ${res.task_id}: ${res.error ?? res.phase}; inspect this task before retrying`,
        );
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
      if (focusedTerm.agent) {
        await lazed.agentPrompt(focusedTerm.pane_id, text);
      } else {
        await lazed.paneSend(focusedTerm.pane_id, `${text}\n`);
      }
      onFlash(`sent to ${focusedTerm.label ?? focusedTerm.pane_id}`);
      await inbox.update(item.id, { status: "done" });
      setMenu(null);
      onChanged();
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const row = (item: InboxItem) => {
    // the source chip only survives when it adds info beyond the section
    // header — "manual" in Captured, or the automation name that IS the group
    const group = providerOf(item);
    const showSource = item.source !== "manual" && item.source.trim() !== group;
    const isDone = item.status === "done";
    return (
      <div key={item.id} className={`ibx-row${isDone ? " done" : ""}`}>
        <div className="ibx-actions ibx-actions-lead">
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
                size={15}
                strokeWidth={1.7}
              />
            </button>
          )}
          <button
            type="button"
            className="ibx-btn"
            title="snooze"
            onClick={(e) => openMenu(e, item, "snooze")}
          >
            <HugeiconsIcon icon={Clock01Icon} size={15} strokeWidth={1.7} />
          </button>
          <button
            type="button"
            className="ibx-btn"
            title="delegate to agent / pane"
            onClick={(e) => openMenu(e, item, "delegate")}
          >
            <HugeiconsIcon icon={SentIcon} size={15} strokeWidth={1.7} />
          </button>
          <button
            type="button"
            className={`ibx-btn ibx-done${isDone ? " on" : ""}`}
            title={isDone ? "reopen" : "mark done"}
            onClick={() =>
              run(
                inbox.update(item.id, {
                  status: isDone ? "open" : "done",
                }),
              )
            }
          >
            <HugeiconsIcon
              icon={CheckmarkCircle02Icon}
              size={15}
              strokeWidth={1.7}
            />
          </button>
        </div>
        <div className="ibx-main">
          <div className="ibx-title" title={item.body ?? item.title}>
            {item.title}
          </div>
          <div className="ibx-meta">
            {showSource && <span className="ibx-source">{item.source}</span>}
            <span>{relTime(item.at)}</span>
            {item.status === "snoozed" && (
              <span className="ibx-until">
                ⏰ {snoozeLabel(item.snooze_until)}
              </span>
            )}
            {isDone && <span className="ibx-done-tag">done</span>}
          </div>
        </div>
      </div>
    );
  };

  return (
    <div className="ibx-screen">
      <div className="ibx-col">
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
          {groups.map((g) => (
            <div key={g.key} className="ibx-group">
              <button
                type="button"
                className="ibx-group-head"
                title={collapsed.has(g.key) ? "expand" : "collapse"}
                onClick={() => toggleGroup(g.key)}
              >
                <span className="ibx-group-caret">
                  {collapsed.has(g.key) ? "▸" : "▾"}
                </span>
                <HugeiconsIcon icon={g.icon} size={13} strokeWidth={1.7} />
                <span className="ibx-group-label">{g.label}</span>
                <span className="ibx-group-count">{g.items.length}</span>
              </button>
              {!collapsed.has(g.key) &&
                (g.key === "jira"
                  ? issueClusters(g.items).map((c) => {
                      if (!c.issue)
                        return (
                          <Fragment key={c.items[0].id}>
                            {c.items.map(row)}
                          </Fragment>
                        );
                      const allDone = c.items.every((i) => i.status === "done");
                      const url = c.items
                        .find((i) => i.url)
                        ?.url?.split("?")[0];
                      return (
                        <div
                          key={c.issue}
                          className={`ibx-issue${allDone ? " all-done" : ""}`}
                        >
                          <div className="ibx-issue-head">
                            <span className="ibx-issue-key">{c.issue}</span>
                            <span className="ibx-issue-count">
                              {c.items.length}
                            </span>
                            {url && (
                              <button
                                type="button"
                                className="ibx-btn"
                                title={`open ${url}`}
                                onClick={() =>
                                  openUrl(url).catch((e) => onError(String(e)))
                                }
                              >
                                <HugeiconsIcon
                                  icon={ArrowUpRight01Icon}
                                  size={12}
                                  strokeWidth={1.7}
                                />
                              </button>
                            )}
                            <button
                              type="button"
                              className={`ibx-btn ibx-done${allDone ? " on" : ""}`}
                              title={allDone ? "reopen all" : "mark all done"}
                              onClick={() =>
                                run(
                                  Promise.all(
                                    c.items.map((i) =>
                                      inbox.update(i.id, {
                                        status: allDone ? "open" : "done",
                                      }),
                                    ),
                                  ),
                                )
                              }
                            >
                              <HugeiconsIcon
                                icon={CheckmarkCircle02Icon}
                                size={13}
                                strokeWidth={1.7}
                              />
                            </button>
                          </div>
                          {c.items.map(row)}
                        </div>
                      );
                    })
                  : g.items.map(row))}
            </div>
          ))}
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
      </div>
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
                      ? `send to ${focusedTerm.label ?? focusedTerm.pane_id}`
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
