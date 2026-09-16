import {
  AlertCircleIcon,
  ArrowDown01Icon,
  ArrowLeft01Icon,
  ArrowRight01Icon,
  CheckmarkCircle02Icon,
  PencilEdit02Icon,
  PlayIcon,
  PlusIcon,
  WorkflowIcon,
  ZapIcon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ask } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { type Automation, automations } from "../shared/automations";

function relTime(at?: number): string {
  if (!at) return "never";
  const s = Math.max(0, Math.floor(Date.now() / 1000) - at);
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}

function everyLabel(secs: number): string {
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.round(secs / 60)}m`;
  return `${Math.round(secs / 3600)}h`;
}

const ACTION_LABEL: Record<string, string> = {
  collect: "collect",
  notify: "notify",
  command: "cmd",
  agent: "agent",
};

function AutomationRow({
  auto,
  expanded,
  onToggleExpand,
  onEdit,
  onChanged,
}: {
  auto: Automation;
  expanded: boolean;
  onToggleExpand: () => void;
  onEdit: () => void;
  onChanged: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const run = (p: Promise<unknown>) =>
    p
      .catch(() => {})
      .finally(() => {
        setBusy(false);
        onChanged();
      });

  const remove = async () => {
    const ok = await ask(
      `Delete automation “${auto.name}”? Its collected items go with it.`,
      {
        title: "Delete Automation",
        kind: "warning",
        okLabel: "Delete",
        cancelLabel: "Cancel",
      },
    ).catch(() => false);
    if (ok) run(automations.delete(auto.id));
  };

  return (
    <div className={`auto-row ${auto.enabled ? "" : "off"}`}>
      <div className="auto-head">
        <button
          type="button"
          className="auto-expand"
          onClick={onToggleExpand}
          title={expanded ? "collapse" : "show items"}
        >
          <HugeiconsIcon
            icon={expanded ? ArrowDown01Icon : ArrowRight01Icon}
            size={11}
            strokeWidth={1.5}
          />
        </button>
        <button
          type="button"
          className={`auto-dot ${auto.enabled ? "on" : ""}`}
          title={
            auto.enabled
              ? "enabled — click to pause"
              : "paused — click to enable"
          }
          onClick={() => run(automations.setEnabled(auto.id, !auto.enabled))}
        />
        <button
          type="button"
          className="auto-name"
          onClick={onToggleExpand}
          title={auto.command}
        >
          {auto.name}
        </button>
        <span className="auto-kind">
          {ACTION_LABEL[auto.action.kind] ?? "?"}
        </span>
        <button
          type="button"
          className="side-close"
          title="run now"
          disabled={busy}
          onClick={() => {
            setBusy(true);
            run(automations.runNow(auto.id));
          }}
        >
          <HugeiconsIcon icon={PlayIcon} size={11} strokeWidth={1.5} />
        </button>
        <button
          type="button"
          className="side-close"
          title="edit"
          onClick={onEdit}
        >
          <HugeiconsIcon icon={PencilEdit02Icon} size={11} strokeWidth={1.5} />
        </button>
        <button
          type="button"
          className="side-close"
          title="delete"
          onClick={remove}
        >
          ✕
        </button>
      </div>
      <div className="auto-meta">
        <span>every {everyLabel(auto.interval_secs)}</span>
        <span>·</span>
        <span>last {relTime(auto.last_ok_at ?? auto.last_run_at)}</span>
        {auto.fire_count > 0 && (
          <>
            <span>·</span>
            <span className="auto-fired">{auto.fire_count} fired</span>
          </>
        )}
        {!auto.seeded && <span>· seeding</span>}
      </div>
      {auto.last_error && (
        <div className="auto-error" title={auto.last_error}>
          <HugeiconsIcon icon={AlertCircleIcon} size={11} strokeWidth={1.5} />
          {auto.last_error}
        </div>
      )}
      {expanded && (
        <div className="auto-items">
          {auto.last_items.length === 0 ? (
            <div className="auto-empty">no items yet</div>
          ) : (
            auto.last_items.map((i) => (
              <div key={i.id} className="auto-item">
                <span className="auto-item-text" title={i.text}>
                  {i.text}
                </span>
                <span className="auto-item-at">{relTime(i.at)}</span>
                {i.fired ? (
                  <HugeiconsIcon
                    icon={CheckmarkCircle02Icon}
                    size={12}
                    strokeWidth={1.5}
                    className="auto-item-done"
                  />
                ) : (
                  <button
                    type="button"
                    className="auto-item-fire"
                    title="run action for this item"
                    onClick={() => run(automations.fire(auto.id, i.id))}
                  >
                    <HugeiconsIcon icon={ZapIcon} size={11} strokeWidth={1.5} />
                  </button>
                )}
              </div>
            ))
          )}
        </div>
      )}
    </div>
  );
}

/** Full-screen automations manager — pollers that watch a command and fire
 * an action per new item. Opened from the rail (⇧⌘2), styled like the
 * Settings screen. */
export function Automations({
  autos,
  onNew,
  onEdit,
  onChanged,
  editorOpen,
  onClose,
}: {
  autos: Automation[];
  onNew: () => void;
  onEdit: (a: Automation) => void;
  onChanged: () => void;
  /** the editor modal is up — Escape must close it, not this screen */
  editorOpen: boolean;
  onClose: () => void;
}) {
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());

  const toggleExpand = (id: string) =>
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !editorOpen) onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose, editorOpen]);

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
          <HugeiconsIcon icon={WorkflowIcon} size={13} strokeWidth={1.5} />
          Automations
        </button>
      </div>
      <div className="set-body">
        <div className="set-col sess-col">
          <div className="sess-title-row">
            <div>
              <div className="set-title">Automations</div>
              <div className="set-sub">
                Pollers that watch a command — Jira, gh, anything — and fire an
                action on each new item.
              </div>
            </div>
            <button type="button" className="set-btn" onClick={onNew}>
              <HugeiconsIcon icon={PlusIcon} size={11} strokeWidth={1.5} />
              New automation
            </button>
          </div>
          <div className="set-section">
            {autos.length === 0 ? (
              <div className="sess-none">
                No automations yet — New automation creates a poller that
                watches a command and fires an action on new items.
              </div>
            ) : (
              autos.map((a) => (
                <div key={a.id} className="set-card auto-card">
                  <AutomationRow
                    auto={a}
                    expanded={expanded.has(a.id)}
                    onToggleExpand={() => toggleExpand(a.id)}
                    onEdit={() => onEdit(a)}
                    onChanged={onChanged}
                  />
                </div>
              ))
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
