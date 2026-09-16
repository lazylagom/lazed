import {
  ArrowLeft01Icon,
  BotIcon,
  KeyboardIcon,
  Notification03Icon,
  RefreshIcon,
  Settings02Icon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useCallback, useEffect, useState } from "react";
import { type AgentDetectResult, lazed } from "../shared/lazed";
import {
  notificationsEnabled,
  resetSidebarWidth,
  setNotificationsEnabled,
} from "../shared/settings";

type Section = "general" | "agents" | "shortcuts";

const SHORTCUT_GROUPS: {
  name: string;
  items: [keys: string, action: string][];
}[] = [
  {
    name: "Terminals",
    items: [
      ["⌘D / ⌘T", "New terminal"],
      ["⌘W", "Close focused terminal"],
      ["⌘]", "Next terminal"],
      ["⌘[", "Previous terminal"],
      ["⌥1–9", "Switch to terminal"],
    ],
  },
  {
    name: "Projects",
    items: [
      ["⇧⌘N", "Import project"],
      ["⌘1–9", "Switch to project"],
      ["⇧⌘]", "Next project"],
      ["⇧⌘[", "Previous project"],
    ],
  },
  {
    name: "Tools",
    items: [
      ["⌘K", "Prompt"],
      ["⇧⌘A", "Start agent"],
      ["⇧⌘F", "Fan-out"],
      ["⇧⌘I", "Inbox"],
      ["⇧⌘2", "Automations"],
      ["⇧⌘3", "Session monitor"],
      ["⌘,", "Settings"],
    ],
  },
  {
    name: "Sidebar",
    items: [["⇧⌘1", "Projects rail"]],
  },
];

function Toggle({
  on,
  onChange,
}: {
  on: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      className={`set-toggle ${on ? "on" : ""}`}
      onClick={() => onChange(!on)}
    >
      <span className="set-toggle-knob" />
    </button>
  );
}

function AgentsPane() {
  const [result, setResult] = useState<AgentDetectResult | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState("");

  const refresh = useCallback(() => {
    setLoading(true);
    setError(null);
    lazed
      .agentDetect()
      .then(setResult)
      .catch((e) => setError(typeof e === "string" ? e : String(e)))
      .finally(() => setLoading(false));
  }, []);

  useEffect(refresh, []);

  const q = filter.trim().toLowerCase();
  const agents = (result?.agents ?? [])
    .filter((a) => !q || a.kind.toLowerCase().includes(q))
    .sort(
      (a, b) =>
        Number(Boolean(b.path)) - Number(Boolean(a.path)) ||
        a.kind.localeCompare(b.kind),
    );
  const installed = (result?.agents ?? []).filter((a) => a.path).length;

  return (
    <div className="set-col">
      <div className="set-title">Agents</div>
      <div className="set-sub">
        {result
          ? `Agent CLIs detected ${
              result.context === "local"
                ? "on this Mac"
                : `on ${result.context}`
            }.`
          : "Agent CLIs lazed can start in a terminal."}
      </div>
      <div className="set-section">
        <div className="set-agents-head">
          <div className="set-section-title">
            {loading
              ? "Checking…"
              : `${installed} of ${result?.agents.length ?? 0} installed`}
          </div>
          <button
            type="button"
            className="set-btn"
            onClick={refresh}
            disabled={loading}
          >
            <HugeiconsIcon icon={RefreshIcon} size={11} strokeWidth={1.5} />
            Re-check
          </button>
        </div>
        <input
          className="set-search"
          placeholder="Filter agents…"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
        />
        {error ? (
          <div className="set-card">
            <div className="set-row">
              <div className="set-row-sub">{error}</div>
            </div>
          </div>
        ) : (
          <div className="set-card">
            {agents.map((a) => (
              <div key={a.kind} className="set-row">
                <div>
                  <div className="set-row-label">{a.kind}</div>
                  {a.path && (
                    <div className="set-row-sub set-mono">{a.path}</div>
                  )}
                </div>
                <span className={`set-chip ${a.path ? "on" : ""}`}>
                  {a.path ? "installed" : "not installed"}
                </span>
              </div>
            ))}
            {agents.length === 0 && (
              <div className="set-row">
                <div className="set-row-sub">
                  {loading ? "checking PATH…" : "no match"}
                </div>
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

export function Settings({ onClose }: { onClose: () => void }) {
  const [section, setSection] = useState<Section>("general");
  const [notify, setNotify] = useState(notificationsEnabled);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const navItem = (id: Section, icon: typeof Settings02Icon, label: string) => (
    <button
      type="button"
      className={`set-nav-item ${section === id ? "sel" : ""}`}
      onClick={() => setSection(id)}
    >
      <HugeiconsIcon icon={icon} size={13} strokeWidth={1.5} />
      {label}
    </button>
  );

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
        <div className="set-nav-group">General</div>
        {navItem("general", Settings02Icon, "General")}
        {navItem("agents", BotIcon, "Agents")}
        <div className="set-nav-group">Interface</div>
        {navItem("shortcuts", KeyboardIcon, "Shortcuts")}
      </div>
      <div className="set-body">
        {section === "general" && (
          <div className="set-col">
            <div className="set-title">General</div>
            <div className="set-sub">Workspace defaults and app behavior.</div>
            <div className="set-section">
              <div className="set-section-title">Notifications</div>
              <div className="set-card">
                <div className="set-row">
                  <div>
                    <div className="set-row-label">
                      <HugeiconsIcon
                        icon={Notification03Icon}
                        size={13}
                        strokeWidth={1.5}
                        className="set-row-ico"
                      />
                      Agent status alerts
                    </div>
                    <div className="set-row-sub">
                      Send a macOS notification when an agent becomes blocked or
                      finishes. Click it to jump to the terminal.
                    </div>
                  </div>
                  <Toggle
                    on={notify}
                    onChange={(v) => {
                      setNotify(v);
                      setNotificationsEnabled(v);
                    }}
                  />
                </div>
              </div>
            </div>
            <div className="set-section">
              <div className="set-section-title">Sidebar</div>
              <div className="set-card">
                <div className="set-row">
                  <div>
                    <div className="set-row-label">Sidebar width</div>
                    <div className="set-row-sub">
                      Restore the projects sidebar to its default width.
                    </div>
                  </div>
                  <button
                    type="button"
                    className="set-btn"
                    onClick={resetSidebarWidth}
                  >
                    Reset
                  </button>
                </div>
              </div>
            </div>
          </div>
        )}
        {section === "agents" && <AgentsPane />}
        {section === "shortcuts" && (
          <div className="set-col">
            <div className="set-title">Shortcuts</div>
            <div className="set-sub">Keyboard shortcuts.</div>
            {SHORTCUT_GROUPS.map((g) => (
              <div key={g.name} className="set-section">
                <div className="set-section-title">{g.name}</div>
                <div className="set-card">
                  {g.items.map(([keys, action]) => (
                    <div key={keys} className="set-row">
                      <div className="set-row-label">{action}</div>
                      <span className="set-key">{keys}</span>
                    </div>
                  ))}
                </div>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
