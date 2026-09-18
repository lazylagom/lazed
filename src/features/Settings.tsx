import {
  ArrowLeft01Icon,
  BotIcon,
  ConnectIcon,
  KeyboardIcon,
  LockKeyholeIcon,
  MultiplicationSignIcon,
  Notification03Icon,
  RefreshIcon,
  Settings02Icon,
  Unlink03Icon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ask } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useState } from "react";
import {
  INTEGRATION_PROVIDERS,
  type IntegrationProviderSpec,
  type IntegrationSiteState,
  type IntegrationState,
  type IntegrationTestResult,
  integrations,
} from "../shared/integrations";
import {
  type AgentDetectResult,
  type SessionStatus,
  lazed,
} from "../shared/lazed";
import {
  notificationsEnabled,
  resetSidebarWidth,
  setNotificationsEnabled,
} from "../shared/settings";

export type Section = "general" | "integrations" | "agents" | "shortcuts";

const SHORTCUT_GROUPS: {
  name: string;
  items: [keys: string, action: string][];
}[] = [
  {
    name: "Navigation",
    items: [
      ["⌘1–9", "Switch workspace"],
      ["⌃1–9", "Switch tab"],
      ["⌘]", "Next pane"],
      ["⌘[", "Previous pane"],
      ["⇧⌘Enter", "Zoom pane (toggle)"],
      ["⇧⌘]", "Next workspace"],
      ["⇧⌘[", "Previous workspace"],
    ],
  },
  {
    name: "Terminals",
    items: [
      ["⌘D", "New pane (split)"],
      ["⌘T", "New tab"],
      ["⌘W", "Close focused pane"],
    ],
  },
  {
    name: "Projects",
    items: [["⇧⌘N", "Import project"]],
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

/** Jira's staircase mark — no brand icon in the set, so it's drawn. */
function JiraMark({ size = 20 }: { size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2.7"
      strokeLinecap="round"
      aria-hidden="true"
    >
      <path d="M18.6 4.4a5.5 5.5 0 0 1-5.5 5.5" />
      <path d="M13.1 9.9a5.5 5.5 0 0 1-5.5 5.5" />
      <path d="M7.6 15.4A5.5 5.5 0 0 1 2.1 20.9" />
    </svg>
  );
}

function hostLabel(base?: string) {
  if (!base) return "";
  try {
    return new URL(base).hostname;
  } catch {
    return base.replace(/^https?:\/\//, "").split("/")[0];
  }
}

/** Connect modal — validates credentials against the provider, then
 * saves a new site (label = the site's own server title). */
function ConnectModal({
  p,
  onClose,
  onConnected,
}: {
  p: IntegrationProviderSpec;
  onClose: () => void;
  onConnected: () => void;
}) {
  const [tab, setTab] = useState(p.tabs[0]?.id ?? "");
  const [draft, setDraft] = useState<Record<string, string>>({});
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const active = p.tabs.find((t) => t.id === tab) ?? p.tabs[0];

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const connect = () => {
    if (!active || busy) return;
    const fields: Record<string, string> = { kind: active.id };
    for (const f of active.fields) {
      if (f.secret) continue;
      const v = (draft[f.key] ?? "").trim();
      if (!v && !f.optional) {
        setErr(`${f.label} is required`);
        return;
      }
      fields[f.key] = v;
    }
    if (!token.trim()) {
      setErr(
        `${active.fields.find((f) => f.secret)?.label ?? "Token"} is required`,
      );
      return;
    }
    setBusy(true);
    setErr(null);
    integrations
      .probe(p.id, fields, token.trim())
      .then((r) => {
        if (!r.ok) {
          setErr(r.error ?? "connection failed");
          return;
        }
        const label = r.title || hostLabel(fields.base) || p.label;
        return integrations
          .save(p.id, null, { ...fields, label }, token.trim())
          .then(() => {
            onConnected();
            onClose();
          });
      })
      .catch((e) => setErr(String(e)))
      .finally(() => setBusy(false));
  };

  return (
    <div className="modal-overlay" onMouseDown={onClose}>
      <div className="modal intgconn" onMouseDown={(e) => e.stopPropagation()}>
        <div className="intgconn-head">
          <span className="intgconn-title">
            {p.modal_title ?? `Connect ${p.label}`}
          </span>
          <button
            type="button"
            className="intgconn-x"
            onClick={onClose}
            aria-label="close"
          >
            <HugeiconsIcon
              icon={MultiplicationSignIcon}
              size={18}
              strokeWidth={1.5}
            />
          </button>
        </div>
        {p.modal_sub && <div className="intgconn-sub">{p.modal_sub}</div>}
        {p.tabs.length > 1 && (
          <div className="intgconn-tabs">
            {p.tabs.map((t) => (
              <button
                key={t.id}
                type="button"
                className={`intgconn-tab ${tab === t.id ? "sel" : ""}`}
                onClick={() => {
                  setTab(t.id);
                  setErr(null);
                }}
              >
                {t.label}
              </button>
            ))}
          </div>
        )}
        <div className="intgconn-fields">
          {active?.fields.map((f) => (
            <label className="set-field-wrap" key={f.key}>
              <span className="set-field-label">{f.label}</span>
              <input
                className="set-field"
                type={f.secret ? "password" : "text"}
                spellCheck={false}
                placeholder={f.placeholder}
                value={f.secret ? token : (draft[f.key] ?? "")}
                onChange={(e) =>
                  f.secret
                    ? setToken(e.target.value)
                    : setDraft((d) => ({ ...d, [f.key]: e.target.value }))
                }
                onKeyDown={(e) => {
                  if (e.key === "Enter") connect();
                }}
              />
              {f.hint && (
                <span className="set-field-hint">
                  {f.hint.text}
                  {f.hint.link && f.hint.url && (
                    <button
                      type="button"
                      className="set-field-link"
                      onClick={() =>
                        invoke("open_url", { url: f.hint?.url }).catch(() => {})
                      }
                    >
                      {f.hint.link}
                    </button>
                  )}
                </span>
              )}
            </label>
          ))}
        </div>
        {p.lock_note && (
          <div className="intgconn-lock">
            <HugeiconsIcon icon={LockKeyholeIcon} size={12} strokeWidth={1.5} />
            <span>{p.lock_note}</span>
          </div>
        )}
        {err && <div className="intgconn-err">{err}</div>}
        <div className="intgconn-actions">
          <button type="button" className="set-btn" onClick={onClose}>
            Cancel
          </button>
          <button
            type="button"
            className="set-btn primary"
            disabled={busy}
            onClick={connect}
          >
            {busy ? "Connecting…" : "Connect"}
          </button>
        </div>
      </div>
    </div>
  );
}

function IntegrationsPane() {
  const [states, setStates] = useState<Record<string, IntegrationState>>({});
  const [testing, setTesting] = useState<string | null>(null);
  const [results, setResults] = useState<
    Record<string, IntegrationTestResult | null>
  >({});
  const [connectFor, setConnectFor] = useState<IntegrationProviderSpec | null>(
    null,
  );
  const [checking, setChecking] = useState(false);

  const refresh = useCallback(() => {
    integrations
      .list()
      .then((list) => {
        const st: Record<string, IntegrationState> = {};
        for (const s of list) st[s.provider] = s;
        setStates(st);
      })
      .catch(() => {});
  }, []);

  useEffect(refresh, []);

  const recheck = () => {
    setChecking(true);
    setResults({});
    refresh();
    setTimeout(() => setChecking(false), 400);
  };

  const siteKey = (p: string, s: string) => `${p}/${s}`;

  const test = (pid: string, s: IntegrationSiteState) => {
    const key = siteKey(pid, s.id);
    setTesting(key);
    integrations
      .test(pid, s.id)
      .then((r) => setResults((res) => ({ ...res, [key]: r })))
      .catch((e) =>
        setResults((r) => ({
          ...r,
          [key]: { ok: false, error: String(e) },
        })),
      )
      .finally(() => setTesting(null));
  };

  const disconnect = async (
    p: IntegrationProviderSpec,
    s: IntegrationSiteState,
  ) => {
    const name = s.label || hostLabel(s.fields.base) || "this site";
    const ok = await ask(
      `Disconnect ${name}? Its stored token is deleted from the Keychain.`,
      {
        title: `Disconnect ${p.label}`,
        kind: "warning",
        okLabel: "Disconnect",
        cancelLabel: "Cancel",
      },
    ).catch(() => false);
    if (!ok) return;
    setResults((r) => ({ ...r, [siteKey(p.id, s.id)]: null }));
    integrations
      .remove(p.id, s.id)
      .then(refresh)
      .catch(() => {});
  };

  return (
    <div className="set-col">
      <div className="set-title">Integrations</div>
      <div className="set-sub">
        Connected accounts. Values are injected as env vars into automation
        shells — a var already exported in your shell environment wins over the
        stored one.
      </div>
      {INTEGRATION_PROVIDERS.map((p) => {
        const st = states[p.id];
        const sites = st?.sites ?? [];
        const connected = st?.configured ?? false;
        return (
          <div className="set-section" key={p.id}>
            <div className="set-card intg-card">
              <div className="intg-head">
                <span className="intg-logo">
                  {p.id === "jira" && <JiraMark />}
                </span>
                <div className="intg-head-main">
                  <div className="intg-title">{p.label}</div>
                  <div className="intg-sub">
                    {connected
                      ? `${sites.length} site${sites.length === 1 ? "" : "s"} connected`
                      : p.sub}
                  </div>
                </div>
                <button
                  type="button"
                  className="set-btn primary"
                  onClick={() => setConnectFor(p)}
                >
                  {connected
                    ? (p.add_label ?? `Add ${p.label} site`)
                    : (p.connect_label ?? `Connect ${p.label}`)}
                </button>
                <span className={`set-chip ${connected ? "on" : "warn"}`}>
                  {connected ? "Connected" : "Not connected"}
                </span>
              </div>
              {p.scope_note && (
                <div className="intg-scope">
                  <div className="intg-scope-title">
                    Account scope: Local Mac
                  </div>
                  <div className="intg-scope-sub">{p.scope_note}</div>
                </div>
              )}
              {sites.length === 0 ? (
                <>
                  {p.blurb && <div className="intg-blurb">{p.blurb}</div>}
                  <div className="intg-more">
                    <button
                      type="button"
                      className="intg-recheck"
                      onClick={recheck}
                      disabled={checking}
                    >
                      {checking ? "Checking…" : "Re-check"}
                    </button>
                  </div>
                </>
              ) : (
                <>
                  <div className="intg-sites">
                    {sites.map((s) => {
                      const key = siteKey(p.id, s.id);
                      const res = results[key];
                      const sub = [
                        s.fields.base,
                        s.fields.email ?? s.fields.username,
                      ]
                        .filter(Boolean)
                        .join(" · ");
                      return (
                        <div className="intg-site" key={s.id}>
                          <div className="intg-site-main">
                            <div className="intg-site-label">
                              {s.label || hostLabel(s.fields.base) || "Site"}
                            </div>
                            <div className="intg-site-sub">{sub}</div>
                            {res && (
                              <div
                                className={`intg-site-note ${res.ok ? "ok" : "err"}`}
                              >
                                {res.ok
                                  ? `Connected — ${res.detail}`
                                  : res.error}
                              </div>
                            )}
                          </div>
                          <button
                            type="button"
                            className="set-btn"
                            disabled={testing === key}
                            onClick={() => test(p.id, s)}
                          >
                            {testing === key ? "Testing…" : "Test"}
                          </button>
                          <button
                            type="button"
                            className="intg-site-x"
                            title="disconnect site"
                            onClick={() => disconnect(p, s)}
                          >
                            <HugeiconsIcon
                              icon={Unlink03Icon}
                              size={13}
                              strokeWidth={1.5}
                            />
                          </button>
                        </div>
                      );
                    })}
                  </div>
                  {p.sites_note && (
                    <div className="intg-foot">{p.sites_note}</div>
                  )}
                </>
              )}
            </div>
          </div>
        );
      })}
      {connectFor && (
        <ConnectModal
          p={connectFor}
          onClose={() => setConnectFor(null)}
          onConnected={refresh}
        />
      )}
    </div>
  );
}

function fmtUptime(startedAt: number | undefined): string {
  if (!startedAt) return "";
  const secs = Math.max(0, Math.floor(Date.now() / 1000) - startedAt);
  if (secs < 60) return `${secs}s`;
  const m = Math.floor(secs / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 48) return `${h}h ${m % 60}m`;
  return `${Math.floor(h / 24)}d ${h % 24}h`;
}

/** Running-daemon card: version / pid / uptime, a stale-binary notice, and
 *  a restart button. Restarting kills the processes inside every pane, so
 *  it always confirms first. */
function DaemonPane() {
  const [status, setStatus] = useState<SessionStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(() => {
    lazed
      .status()
      .then((s) => {
        setStatus(s);
        setError(null);
      })
      .catch((e) => {
        setStatus(null);
        setError(String(e));
      });
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const restart = async () => {
    const terms = status?.terms ?? 0;
    const ok = await ask(
      `Restart the lazed daemon?\n\nPanes are restored from the saved session, but anything running inside them (${terms} pane${terms === 1 ? "" : "s"}, including agents) will be terminated.`,
      { title: "Restart daemon", kind: "warning", okLabel: "Restart" },
    );
    if (!ok) return;
    setBusy(true);
    setError(null);
    try {
      const s = await lazed.restartServer();
      setStatus(s);
    } catch (e) {
      setError(String(e));
      refresh();
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="set-section">
      <div className="set-section-title">Daemon</div>
      <div className="set-card">
        <div className="set-row">
          <div>
            <div className="set-row-label">lazed server</div>
            <div className="set-row-sub">
              {status
                ? `v${status.version ?? "?"} · pid ${status.pid ?? "?"} · up ${fmtUptime(status.started_at)} · ${status.terms ?? 0} panes`
                : error
                  ? `not reachable: ${error}`
                  : "checking…"}
            </div>
            {status?.binary_updated && (
              <div className="set-row-sub set-warn">
                The lazed binary on disk is newer than the running daemon.
                Restart to pick up the new build.
              </div>
            )}
            {status && error && (
              <div className="set-row-sub set-warn">{error}</div>
            )}
          </div>
          <button
            type="button"
            className={`set-btn ${status?.binary_updated ? "primary" : ""}`}
            onClick={restart}
            disabled={busy}
          >
            {busy ? "Restarting…" : "Restart"}
          </button>
        </div>
      </div>
    </div>
  );
}

export function Settings({
  initial,
  onClose,
}: {
  initial?: Section;
  onClose: () => void;
}) {
  const [section, setSection] = useState<Section>(initial ?? "general");
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
        {navItem("integrations", ConnectIcon, "Integrations")}
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
            <DaemonPane />
          </div>
        )}
        {section === "integrations" && <IntegrationsPane />}
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
