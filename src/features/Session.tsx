import {
  Activity01Icon,
  ArrowLeft01Icon,
  BotIcon,
  FolderGitIcon,
  GitBranchIcon,
  RefreshIcon,
  TerminalIcon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useCallback, useEffect, useState } from "react";
import {
  type AgentStatus,
  type SessionStatus,
  type Snapshot,
  type TerminalInfo,
  lazed,
} from "../shared/lazed";

function basename(p?: string) {
  if (!p) return "";
  const parts = p.replace(/\/$/, "").split("/");
  return parts[parts.length - 1] || p;
}

const STATUS_ORDER: AgentStatus[] = [
  "working",
  "blocked",
  "done",
  "idle",
  "unknown",
];

/** Full-screen session monitor — daemon liveness plus every project's
 * terminals with their live agent status. Opened from the rail (⇧⌘3),
 * styled like the Settings screen. */
export function Session({
  snap,
  focusedTermId,
  onJumpTerm,
  onFocusProject,
  onClose,
}: {
  snap: Snapshot | null;
  focusedTermId: string | null;
  onJumpTerm: (termId: string, projectId: string) => void;
  onFocusProject: (id: string) => void;
  onClose: () => void;
}) {
  const [status, setStatus] = useState<SessionStatus | null>(null);
  const [statusErr, setStatusErr] = useState<string | null>(null);

  const refresh = useCallback(() => {
    lazed
      .status()
      .then((s) => {
        setStatus(s);
        setStatusErr(null);
      })
      .catch((e) => setStatusErr(String(e)));
  }, []);

  useEffect(() => {
    refresh();
    const t = window.setInterval(refresh, 5000);
    return () => window.clearInterval(t);
  }, [refresh]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const projects = snap?.projects ?? [];
  const termsById = new Map((snap?.terminals ?? []).map((t) => [t.term_id, t]));
  const allTerms = snap?.terminals ?? [];

  const counts = new Map<AgentStatus, number>();
  let agentTotal = 0;
  for (const t of allTerms) {
    if (!t.agent_kind) continue;
    agentTotal += 1;
    const s = t.agent_status ?? "unknown";
    counts.set(s, (counts.get(s) ?? 0) + 1);
  }
  const deadCount = allTerms.filter((t) => t.dead).length;

  const termName = (t: TerminalInfo) =>
    t.label ?? t.agent_kind ?? basename(t.cwd) ?? t.term_id;

  const jump = (t: TerminalInfo, projectId: string) => {
    onJumpTerm(t.term_id, projectId);
    onClose();
  };

  const daemonState = statusErr
    ? "blocked"
    : status?.running
      ? "done"
      : "unknown";

  return (
    <div
      className="settings"
      data-tauri-drag-region
      onMouseDown={(e) => {
        if (e.button === 0 && e.target === e.currentTarget) {
          getCurrentWindow()
            .startDragging()
            .catch(() => {});
        }
      }}
    >
      <div className="set-nav">
        <button type="button" className="set-back" onClick={onClose}>
          <HugeiconsIcon icon={ArrowLeft01Icon} size={13} strokeWidth={1.5} />
          Back to app
        </button>
        <div className="set-nav-group">Monitor</div>
        <button type="button" className="set-nav-item sel">
          <HugeiconsIcon icon={Activity01Icon} size={13} strokeWidth={1.5} />
          Session
        </button>
      </div>
      <div className="set-body">
        <div className="set-col sess-col">
          <div className="sess-title-row">
            <div>
              <div className="set-title">Session</div>
              <div className="set-sub">
                Live view of the lazed daemon — projects, terminals, and agent
                states.
              </div>
            </div>
            <button type="button" className="set-btn" onClick={refresh}>
              <HugeiconsIcon icon={RefreshIcon} size={11} strokeWidth={1.5} />
              Refresh
            </button>
          </div>
          <div className="set-section">
            <div className="set-section-title">Daemon</div>
            <div className="set-card">
              <div className="set-row">
                <div>
                  <div className="set-row-label">
                    <span className={`dot ${daemonState}`} />
                    daemon
                  </div>
                  <div className="set-row-sub">
                    {statusErr ?? (status ? "running" : "connecting…")}
                  </div>
                </div>
                {status?.version && (
                  <span className="sess-val">v{status.version}</span>
                )}
              </div>
              <div className="set-row">
                <div className="set-row-label">Projects</div>
                <span className="sess-val">{projects.length}</span>
              </div>
              <div className="set-row">
                <div className="set-row-label">Terminals</div>
                <span className="sess-val">
                  {allTerms.length}
                  {deadCount > 0 && ` · ${deadCount} dead`}
                </span>
              </div>
              <div className="set-row">
                <div className="set-row-label">Agents</div>
                {agentTotal > 0 ? (
                  <span className="sess-chips">
                    {STATUS_ORDER.filter((s) => counts.get(s)).map((s) => (
                      <span key={s} className={`badge ${s}`}>
                        {s} {counts.get(s)}
                      </span>
                    ))}
                  </span>
                ) : (
                  <span className="sess-val">none running</span>
                )}
              </div>
            </div>
          </div>
          <div className="set-section">
            <div className="set-section-title">Projects</div>
            {projects.length === 0 && (
              <div className="sess-none">no projects in this session</div>
            )}
            {projects.map((p) => {
              const terms = p.terminals
                .map((id) => termsById.get(id))
                .filter((t): t is TerminalInfo => Boolean(t));
              const name = p.label ?? basename(p.repo_root) ?? p.project_id;
              return (
                <div key={p.project_id} className="set-card sess-proj">
                  <button
                    type="button"
                    className="sess-proj-head"
                    title={`${p.repo_root} — click to focus`}
                    onClick={() => {
                      onFocusProject(p.project_id);
                      onClose();
                    }}
                  >
                    <HugeiconsIcon
                      icon={FolderGitIcon}
                      size={13}
                      strokeWidth={1.5}
                      className="set-row-ico"
                    />
                    <span className="sess-proj-name">{name}</span>
                    <span className="sess-dim">
                      {p.project_id}
                      {p.focused ? " · focused" : ""}
                    </span>
                  </button>
                  {terms.length === 0 && (
                    <div className="sess-none">no terminals</div>
                  )}
                  {terms.map((t) => {
                    const st = t.agent_status ?? "unknown";
                    const meta = [
                      t.term_id,
                      t.kind === "worktree" ? (t.branch ?? "worktree") : t.kind,
                      t.agent_kind,
                      t.cols && t.rows ? `${t.cols}×${t.rows}` : null,
                      t.dead ? "dead" : null,
                    ]
                      .filter(Boolean)
                      .join(" · ");
                    return (
                      <button
                        key={t.term_id}
                        type="button"
                        className={`sess-term ${
                          t.term_id === focusedTermId ? "focused" : ""
                        }`}
                        title={`${t.cwd}\n${t.command ?? ""}${
                          t.scroll
                            ? `\nscroll ${t.scroll.offset_from_bottom}/${t.scroll.max_offset_from_bottom}`
                            : ""
                        }`}
                        onClick={() => jump(t, p.project_id)}
                      >
                        <span className={`dot ${st}`} />
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
                          className="set-row-ico"
                        />
                        <span className="sess-term-main">
                          <span className={`sess-term-name st-${st}`}>
                            {termName(t)}
                          </span>
                          <span className="sess-term-sub">{t.cwd}</span>
                        </span>
                        <span className="sess-dim">{meta}</span>
                      </button>
                    );
                  })}
                </div>
              );
            })}
          </div>
        </div>
      </div>
    </div>
  );
}
