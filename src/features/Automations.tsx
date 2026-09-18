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
import {
  AUTOMATION_PRESETS,
  type Automation,
  type AutomationPreset,
  automationForPreset,
  automations,
  presetInput,
} from "../shared/automations";
import {
  BIN_INSTALL_HINTS,
  type DepsCheckResult,
  ENV_PROVIDER_PREFIX,
  depsCheck,
} from "../shared/integrations";

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
  inbox: "inbox",
  notify: "notify",
  command: "cmd",
  agent: "agent",
};

/** Ready-made automations — every preset except `custom`. */
const CATALOG = AUTOMATION_PRESETS.filter((p) => p.command);

function Toggle({
  on,
  busy,
  title,
  onToggle,
}: {
  on: boolean;
  busy?: boolean;
  title: string;
  onToggle: () => void;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      className={`set-toggle ${on ? "on" : ""}`}
      title={title}
      disabled={busy}
      onClick={onToggle}
    >
      <span className="set-toggle-knob" />
    </button>
  );
}

/** Collected items + last error — shared by catalog and custom rows. */
function ItemsList({
  auto,
  onFire,
}: {
  auto: Automation;
  onFire: (itemId: string) => void;
}) {
  return (
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
                onClick={() => onFire(i.id)}
              >
                <HugeiconsIcon icon={ZapIcon} size={11} strokeWidth={1.5} />
              </button>
            )}
          </div>
        ))
      )}
    </div>
  );
}

/** One ready-made automation: name + description, an on/off switch, and —
 * once it exists — the usual poller status, items and an edit button. */
function CatalogRow({
  preset,
  auto,
  expanded,
  onToggleExpand,
  onEdit,
  onChanged,
  onOpenIntegrations,
}: {
  preset: AutomationPreset;
  auto?: Automation;
  expanded: boolean;
  onToggleExpand: () => void;
  onEdit: (a: Automation) => void;
  onChanged: () => void;
  onOpenIntegrations?: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [deps, setDeps] = useState<DepsCheckResult | null>(null);
  const on = auto?.enabled ?? false;

  useEffect(() => {
    if (!preset.requires) return;
    depsCheck(preset.requires)
      .then(setDeps)
      .catch(() => {});
  }, [preset]);

  const run = (p: Promise<unknown>) => {
    setBusy(true);
    setError(null);
    p.catch((e) => setError(String(e))).finally(() => {
      setBusy(false);
      onChanged();
    });
  };

  const toggle = () => {
    if (auto) run(automations.setEnabled(auto.id, !auto.enabled));
    else run(automations.save(presetInput(preset)));
  };

  const missingBins = Object.entries(deps?.bins ?? {})
    .filter(([, path]) => !path)
    .map(([name]) => name);
  const missingEnv = Object.entries(deps?.env ?? {})
    .filter(([, ok]) => !ok)
    .map(([name]) => name);
  const provider = missingEnv
    .map((v) => ENV_PROVIDER_PREFIX.find(([prefix]) => v.startsWith(prefix)))
    .find(Boolean)?.[1];
  const missing = missingBins.length > 0 || missingEnv.length > 0;

  return (
    <div
      className={`auto-row auto-catalog ${on ? "" : "off"}`}
      data-preset={preset.id}
    >
      <div className="auto-head">
        <button
          type="button"
          className="auto-expand"
          onClick={onToggleExpand}
          title={expanded ? "collapse" : "show items"}
          disabled={!auto}
          style={auto ? undefined : { visibility: "hidden" }}
        >
          <HugeiconsIcon
            icon={expanded ? ArrowDown01Icon : ArrowRight01Icon}
            size={11}
            strokeWidth={1.5}
          />
        </button>
        <span className={`auto-dot ${on ? "on" : ""}`} aria-hidden />
        <div className="auto-catalog-text">
          <button
            type="button"
            className="auto-name"
            onClick={auto ? onToggleExpand : toggle}
            title={preset.command}
          >
            {auto?.name ?? preset.name.split(" — ")[0]}
          </button>
          {preset.sub && <div className="auto-catalog-sub">{preset.sub}</div>}
        </div>
        {auto && (
          <span className="auto-kind">
            {ACTION_LABEL[auto.action.kind] ?? "?"}
          </span>
        )}
        {auto && (
          <>
            <button
              type="button"
              className="side-close"
              title="run now"
              disabled={busy}
              onClick={() => run(automations.runNow(auto.id))}
            >
              <HugeiconsIcon icon={PlayIcon} size={11} strokeWidth={1.5} />
            </button>
            <button
              type="button"
              className="side-close"
              title="edit"
              onClick={() => onEdit(auto)}
            >
              <HugeiconsIcon
                icon={PencilEdit02Icon}
                size={11}
                strokeWidth={1.5}
              />
            </button>
          </>
        )}
        <Toggle
          on={on}
          busy={busy}
          title={
            on
              ? "on — click to pause"
              : auto
                ? "paused — click to enable"
                : "off — click to switch on"
          }
          onToggle={toggle}
        />
      </div>
      {auto ? (
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
          {!auto.seeded && auto.enabled && <span>· seeding</span>}
        </div>
      ) : (
        missing && (
          <div className="auto-meta auto-catalog-missing">
            <HugeiconsIcon icon={AlertCircleIcon} size={11} strokeWidth={1.5} />
            <span>
              {missingBins.map((b) =>
                BIN_INSTALL_HINTS[b]
                  ? `${b} not found — install: ${BIN_INSTALL_HINTS[b]}`
                  : `${b} not found on PATH`,
              )}
              {missingBins.length > 0 && missingEnv.length > 0 && " · "}
              {missingEnv.length > 0 &&
                (provider
                  ? `connect ${provider} in Settings → Integrations`
                  : `${missingEnv.join(", ")} not set`)}
            </span>
            {provider && onOpenIntegrations && (
              <button
                type="button"
                className="autoedit-dep-link"
                onClick={onOpenIntegrations}
              >
                open integrations
              </button>
            )}
          </div>
        )
      )}
      {(error ?? auto?.last_error) && (
        <div className="auto-error" title={error ?? auto?.last_error}>
          <HugeiconsIcon icon={AlertCircleIcon} size={11} strokeWidth={1.5} />
          {error ?? auto?.last_error}
        </div>
      )}
      {auto && expanded && (
        <ItemsList
          auto={auto}
          onFire={(itemId) => run(automations.fire(auto.id, itemId))}
        />
      )}
    </div>
  );
}

/** A hand-made automation — full controls incl. delete. */
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
        <span className={`auto-dot ${auto.enabled ? "on" : ""}`} aria-hidden />
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
        <Toggle
          on={auto.enabled}
          busy={busy}
          title={
            auto.enabled ? "on — click to pause" : "paused — click to enable"
          }
          onToggle={() => {
            setBusy(true);
            run(automations.setEnabled(auto.id, !auto.enabled));
          }}
        />
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
        <ItemsList
          auto={auto}
          onFire={(itemId) => run(automations.fire(auto.id, itemId))}
        />
      )}
    </div>
  );
}

/** Full-screen automations manager. The top card is a catalog of
 * ready-made automations (Jira mention @me, review requests, …) — each is
 * an on/off switch: on creates the poller from the preset's defaults and
 * starts it, off pauses it. Hand-made pollers live below with the full
 * editor. Opened from the rail (⇧⌘2), styled like the Settings screen. */
export function Automations({
  autos,
  onNew,
  onEdit,
  onChanged,
  editorOpen,
  onClose,
  onOpenIntegrations,
}: {
  autos: Automation[];
  onNew: () => void;
  onEdit: (a: Automation) => void;
  onChanged: () => void;
  /** the editor modal is up — Escape must close it, not this screen */
  editorOpen: boolean;
  onClose: () => void;
  /** jump to Settings → Integrations (offered when a preset needs env vars) */
  onOpenIntegrations?: () => void;
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

  const catalog = CATALOG.map((p) => ({
    preset: p,
    auto: automationForPreset(p, autos),
  }));
  const catalogIds = new Set(
    catalog.map((c) => c.auto?.id).filter((id): id is string => !!id),
  );
  const custom = autos.filter((a) => !catalogIds.has(a.id));

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
                Switch on what to watch — new items land in the inbox. Edit one
                to change how often it polls or what it does per item.
              </div>
            </div>
            <button type="button" className="set-btn" onClick={onNew}>
              <HugeiconsIcon icon={PlusIcon} size={11} strokeWidth={1.5} />
              Custom automation
            </button>
          </div>

          <div className="set-section">
            <div className="set-section-title">Ready-made</div>
            <div className="set-card auto-card">
              {catalog.map(({ preset, auto }) => (
                <CatalogRow
                  key={preset.id}
                  preset={preset}
                  auto={auto}
                  expanded={!!auto && expanded.has(auto.id)}
                  onToggleExpand={() => auto && toggleExpand(auto.id)}
                  onEdit={onEdit}
                  onChanged={onChanged}
                  onOpenIntegrations={onOpenIntegrations}
                />
              ))}
            </div>
          </div>

          {custom.length > 0 && (
            <div className="set-section">
              <div className="set-section-title">Custom</div>
              {custom.map((a) => (
                <div key={a.id} className="set-card auto-card">
                  <AutomationRow
                    auto={a}
                    expanded={expanded.has(a.id)}
                    onToggleExpand={() => toggleExpand(a.id)}
                    onEdit={() => onEdit(a)}
                    onChanged={onChanged}
                  />
                </div>
              ))}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
