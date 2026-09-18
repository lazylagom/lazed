import {
  AlertCircleIcon,
  BotIcon,
  CheckmarkCircle02Icon,
  Download01Icon,
  InboxIcon,
  MultiplicationSignIcon,
  Notification03Icon,
  PlayIcon,
  TerminalIcon,
  WorkflowIcon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { useEffect, useRef, useState } from "react";
import {
  AUTOMATION_PRESETS,
  type ActionKind,
  type Automation,
  type AutomationInput,
  type TestResult,
  automations,
} from "../shared/automations";
import {
  BIN_INSTALL_HINTS,
  type DepsCheckResult,
  ENV_PROVIDER_PREFIX,
  depsCheck,
} from "../shared/integrations";
import { AGENT_KINDS, type ProjectInfo } from "../shared/lazed";

const ACTION_KINDS: {
  id: ActionKind;
  label: string;
  sub: string;
  icon: typeof BotIcon;
}[] = [
  {
    id: "collect",
    label: "Collect",
    sub: "List items on the automation — fire an action manually per item.",
    icon: Download01Icon,
  },
  {
    id: "inbox",
    label: "Inbox",
    sub: "Drop each new item into the inbox for triage.",
    icon: InboxIcon,
  },
  {
    id: "notify",
    label: "Notify",
    sub: "Send a macOS notification for each new item.",
    icon: Notification03Icon,
  },
  {
    id: "command",
    label: "Command",
    sub: "Run a shell template per item — {id} and {text} substituted.",
    icon: TerminalIcon,
  },
  {
    id: "agent",
    label: "Agent",
    sub: "Open a terminal in a project and hand the item to an agent.",
    icon: BotIcon,
  },
];

function intervalToParts(secs: number): { val: string; unit: "m" | "h" } {
  if (secs >= 3600 && secs % 3600 === 0)
    return { val: String(secs / 3600), unit: "h" };
  return { val: String(Math.max(1, Math.round(secs / 60))), unit: "m" };
}

export function AutomationEditor({
  initial,
  projects,
  onSave,
  onDeleteSeen,
  onClose,
  onOpenIntegrations,
}: {
  /** set when editing; undefined = new automation */
  initial?: Automation;
  projects: ProjectInfo[];
  onSave: (input: AutomationInput) => void;
  /** clear seen ids so the next poll re-fires current items */
  onDeleteSeen: (id: string) => void;
  onClose: () => void;
  /** jump to Settings → Integrations (offered when a preset needs env vars) */
  onOpenIntegrations?: () => void;
}) {
  const [name, setName] = useState(initial?.name ?? "");
  const [enabled, setEnabled] = useState(initial?.enabled ?? true);
  const init = intervalToParts(initial?.interval_secs ?? 300);
  const [intervalVal, setIntervalVal] = useState(init.val);
  const [intervalUnit, setIntervalUnit] = useState<"m" | "h">(init.unit);
  const [command, setCommand] = useState(initial?.command ?? "");
  const [presetId, setPresetId] = useState<string | null>(
    () =>
      initial?.preset ??
      AUTOMATION_PRESETS.find(
        (p) => p.command && p.command === initial?.command,
      )?.id ??
      null,
  );
  const [actionKind, setActionKind] = useState<ActionKind>(
    initial?.action.kind ?? "notify",
  );
  const [actionCmd, setActionCmd] = useState(
    initial?.action.kind === "command" ? (initial.action.command ?? "") : "",
  );
  const [agentKind, setAgentKind] = useState(
    initial?.action.kind === "agent"
      ? (initial.action.agent_kind ?? "claude")
      : "claude",
  );
  const [agentProject, setAgentProject] = useState(
    initial?.action.kind === "agent"
      ? (initial.action.project_id ?? "")
      : (projects[0]?.project_id ?? ""),
  );
  const [prompt, setPrompt] = useState(
    initial?.action.kind === "agent"
      ? (initial.action.prompt ?? "")
      : "This item was flagged by an automation. Look into it and report what needs attention:\n{text}",
  );
  const [presetHint, setPresetHint] = useState("");
  const [deps, setDeps] = useState<DepsCheckResult | null>(null);
  const [test, setTest] = useState<TestResult | null>(null);
  const [testing, setTesting] = useState(false);
  const nameRef = useRef<HTMLInputElement>(null);
  const cmdRef = useRef<HTMLTextAreaElement>(null);

  // mount-only: presetId is fixed at construction
  // biome-ignore lint/correctness/useExhaustiveDependencies: intentional mount-only check
  useEffect(() => {
    nameRef.current?.focus();
    const p = AUTOMATION_PRESETS.find((x) => x.id === presetId);
    if (p?.requires)
      depsCheck(p.requires)
        .then(setDeps)
        .catch(() => {});
  }, []);

  const applyPreset = (id: string) => {
    const p = AUTOMATION_PRESETS.find((x) => x.id === id);
    if (!p) return;
    if (p.command) setCommand(p.command);
    if (!name.trim() && p.id !== "custom") setName(p.name.split(" — ")[0]);
    setPresetHint(p.hint);
    setPresetId(id);
    setDeps(null);
    if (p.requires)
      depsCheck(p.requires)
        .then(setDeps)
        .catch(() => {});
    if (!p.command) cmdRef.current?.focus();
  };

  const runTest = () => {
    if (!command.trim() || testing) return;
    setTesting(true);
    setTest(null);
    automations
      .test(command)
      .then(setTest)
      .catch((e) => setTest({ ok: false, error: String(e) }))
      .finally(() => setTesting(false));
  };

  const submit = () => {
    const unit = intervalUnit === "h" ? 3600 : 60;
    const secs = Math.max(15, Math.round(Number(intervalVal) * unit) || 300);
    onSave({
      id: initial?.id,
      name: name.trim() || "automation",
      enabled,
      interval_secs: secs,
      command: command.trim(),
      action:
        actionKind === "command"
          ? { kind: "command", command: actionCmd }
          : actionKind === "agent"
            ? {
                kind: "agent",
                agent_kind: agentKind,
                project_id: agentProject,
                prompt,
              }
            : { kind: actionKind },
      // a catalog automation stays linked to its preset even after its
      // command is edited; a fresh one links to the chip it started from
      preset:
        initial?.preset ??
        (presetId && presetId !== "custom" ? presetId : undefined),
    });
  };

  const canSave =
    command.trim().length > 0 &&
    (actionKind !== "command" || actionCmd.trim().length > 0) &&
    (actionKind !== "agent" ||
      (agentProject.length > 0 && prompt.trim().length > 0));

  const action =
    ACTION_KINDS.find((k) => k.id === actionKind) ?? ACTION_KINDS[0];
  const testItems = test?.items ?? [];
  const missingBins = Object.entries(deps?.bins ?? {})
    .filter(([, path]) => !path)
    .map(([name]) => name);
  const missingEnv = Object.entries(deps?.env ?? {})
    .filter(([, ok]) => !ok)
    .map(([name]) => name);

  return (
    <div className="modal-overlay" onMouseDown={onClose}>
      <div
        className="modal autoedit"
        onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          if (e.key === "Escape") onClose();
          if ((e.metaKey || e.ctrlKey) && e.key === "Enter" && canSave)
            submit();
        }}
      >
        <div className="autoedit-head">
          <HugeiconsIcon
            icon={WorkflowIcon}
            size={14}
            strokeWidth={1.5}
            className="autoedit-head-ico"
          />
          <span className="autoedit-title">
            {initial ? "Edit automation" : "New automation"}
          </span>
          <button
            type="button"
            className="autoedit-x"
            onClick={onClose}
            aria-label="close"
          >
            <HugeiconsIcon
              icon={MultiplicationSignIcon}
              size={16}
              strokeWidth={1.5}
            />
          </button>
        </div>

        <div className="autoedit-body">
          <div className="autoedit-sec">
            <div className="autoedit-flabel">name</div>
            <input
              ref={nameRef}
              className="autoedit-field"
              placeholder="e.g. jira mentions, review requests…"
              value={name}
              onChange={(e) => setName(e.target.value)}
            />
          </div>

          <div className="autoedit-sec">
            <div className="autoedit-sec-head">
              <span className="autoedit-step">1</span>
              <div>
                <div className="autoedit-sec-title">Watch a command</div>
                <div className="autoedit-sec-sub">
                  Polled on an interval — each stdout line becomes an item.
                </div>
              </div>
            </div>
            <div className="autoedit-presets">
              {AUTOMATION_PRESETS.map((p) => (
                <button
                  key={p.id}
                  type="button"
                  className={`fanout-kind ${presetId === p.id ? "sel" : ""}`}
                  title={p.name}
                  onClick={() => applyPreset(p.id)}
                >
                  {p.tag}
                </button>
              ))}
            </div>
            <textarea
              ref={cmdRef}
              className="autoedit-field autoedit-mono autoedit-cmd"
              placeholder={"shell command whose stdout lists items…"}
              value={command}
              onChange={(e) => {
                setCommand(e.target.value);
                setPresetId(null);
              }}
              rows={3}
              spellCheck={false}
            />
            {presetHint && <div className="autoedit-hint">{presetHint}</div>}
            {(missingBins.length > 0 || missingEnv.length > 0) && (
              <div className="autoedit-deps">
                {missingBins.map((b) => (
                  <div key={b} className="autoedit-dep">
                    <HugeiconsIcon
                      icon={AlertCircleIcon}
                      size={11}
                      strokeWidth={1.5}
                      className="autoedit-dep-ico"
                    />
                    <span>
                      <code>{b}</code> not found
                      {BIN_INSTALL_HINTS[b]
                        ? ` — install: ${BIN_INSTALL_HINTS[b]}`
                        : " on PATH"}
                    </span>
                  </div>
                ))}
                {missingEnv.map((v) => {
                  const provider = ENV_PROVIDER_PREFIX.find(([prefix]) =>
                    v.startsWith(prefix),
                  )?.[1];
                  return (
                    <div key={v} className="autoedit-dep">
                      <HugeiconsIcon
                        icon={AlertCircleIcon}
                        size={11}
                        strokeWidth={1.5}
                        className="autoedit-dep-ico"
                      />
                      <span>
                        <code>{v}</code> not set
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
                  );
                })}
              </div>
            )}
            <div className="autoedit-sched">
              <span className="autoedit-sched-label">every</span>
              <input
                className="autoedit-field autoedit-min"
                type="number"
                min={1}
                value={intervalVal}
                onChange={(e) => setIntervalVal(e.target.value)}
              />
              <select
                className="autoedit-field autoedit-unit"
                value={intervalUnit}
                onChange={(e) => setIntervalUnit(e.target.value as "m" | "h")}
              >
                <option value="m">min</option>
                <option value="h">hr</option>
              </select>
              <button
                type="button"
                className="autoedit-test"
                disabled={testing || !command.trim()}
                onClick={runTest}
              >
                <HugeiconsIcon icon={PlayIcon} size={10} strokeWidth={1.5} />
                {testing ? "running…" : "test"}
              </button>
              <div className="autoedit-enabled">
                <span>enabled</span>
                <button
                  type="button"
                  role="switch"
                  aria-checked={enabled}
                  className={`set-toggle ${enabled ? "on" : ""}`}
                  onClick={() => setEnabled(!enabled)}
                >
                  <span className="set-toggle-knob" />
                </button>
              </div>
            </div>
            {test && (
              <div className={`autoedit-testres ${test.ok ? "ok" : "err"}`}>
                <div className="autoedit-testres-head">
                  <HugeiconsIcon
                    icon={test.ok ? CheckmarkCircle02Icon : AlertCircleIcon}
                    size={12}
                    strokeWidth={1.5}
                  />
                  {test.ok
                    ? testItems.length === 0
                      ? "command ran — no items in output"
                      : `${testItems.length} item${testItems.length === 1 ? "" : "s"}`
                    : "command failed"}
                </div>
                {test.ok ? (
                  testItems.slice(0, 5).map((i) => (
                    <div key={i.id} className="autoedit-testitem">
                      {i.text}
                    </div>
                  ))
                ) : (
                  <div className="autoedit-testitem">{test.error}</div>
                )}
              </div>
            )}
            {!initial && (
              <div className="autoedit-hint">
                the first successful poll only records items — actions fire on
                items that appear afterwards
              </div>
            )}
          </div>

          <div className="autoedit-sec">
            <div className="autoedit-sec-head">
              <span className="autoedit-step">2</span>
              <div>
                <div className="autoedit-sec-title">For each new item</div>
                <div className="autoedit-sec-sub">{action.sub}</div>
              </div>
            </div>
            <div className="autoedit-actions">
              {ACTION_KINDS.map((k) => (
                <button
                  key={k.id}
                  type="button"
                  className={`autoedit-action ${actionKind === k.id ? "sel" : ""}`}
                  onClick={() => setActionKind(k.id)}
                >
                  <HugeiconsIcon icon={k.icon} size={15} strokeWidth={1.5} />
                  <span>{k.label}</span>
                </button>
              ))}
            </div>
            {actionKind === "command" && (
              <>
                <input
                  className="autoedit-field autoedit-mono"
                  placeholder={
                    "command template — e.g. open https://you.atlassian.net/browse/{id}"
                  }
                  value={actionCmd}
                  onChange={(e) => setActionCmd(e.target.value)}
                  spellCheck={false}
                />
                <div className="autoedit-hint">
                  {"{id} and {text} are substituted shell-escaped"}
                </div>
              </>
            )}
            {actionKind === "agent" && (
              <>
                <div className="autoedit-row">
                  <select
                    className="autoedit-field"
                    value={agentProject}
                    onChange={(e) => setAgentProject(e.target.value)}
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
                    className="autoedit-field autoedit-kind"
                    value={agentKind}
                    onChange={(e) => setAgentKind(e.target.value)}
                  >
                    {AGENT_KINDS.map((k) => (
                      <option key={k} value={k}>
                        {k}
                      </option>
                    ))}
                  </select>
                </div>
                <textarea
                  className="autoedit-field autoedit-mono autoedit-cmd"
                  placeholder={"prompt template — {id} {text} substituted raw"}
                  value={prompt}
                  onChange={(e) => setPrompt(e.target.value)}
                  rows={3}
                  spellCheck={false}
                />
                <div className="autoedit-hint">
                  opens a new terminal in the project, starts the agent, then
                  sends this prompt
                </div>
              </>
            )}
          </div>
        </div>

        <div className="autoedit-footer">
          {initial && (
            <button
              type="button"
              className="autoedit-reset"
              title="forget seen ids — next poll re-fires current items"
              onClick={() => onDeleteSeen(initial.id)}
            >
              reset seen
            </button>
          )}
          <span className="autoedit-kbd">⌘↵ save</span>
          <button type="button" className="autoedit-cancel" onClick={onClose}>
            cancel
          </button>
          <button
            type="button"
            className="autoedit-save"
            disabled={!canSave}
            onClick={submit}
          >
            {initial ? "save changes" : "create automation"}
          </button>
        </div>
      </div>
    </div>
  );
}
