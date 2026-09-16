import { useEffect, useRef, useState } from "react";
import {
  AUTOMATION_PRESETS,
  type ActionKind,
  type Automation,
  type AutomationInput,
  type TestResult,
  automations,
} from "../shared/automations";
import { AGENT_KINDS, type ProjectInfo } from "../shared/lazed";

const ACTION_KINDS: { id: ActionKind; label: string; sub: string }[] = [
  { id: "collect", label: "Collect", sub: "list items, fire manually" },
  { id: "notify", label: "Notify", sub: "macOS notification per item" },
  { id: "command", label: "Run command", sub: "shell template per item" },
  { id: "agent", label: "Spawn agent", sub: "new terminal → agent → prompt" },
];

export function AutomationEditor({
  initial,
  projects,
  onSave,
  onDeleteSeen,
  onClose,
}: {
  /** set when editing; undefined = new automation */
  initial?: Automation;
  projects: ProjectInfo[];
  onSave: (input: AutomationInput) => void;
  /** clear seen ids so the next poll re-fires current items */
  onDeleteSeen: (id: string) => void;
  onClose: () => void;
}) {
  const [name, setName] = useState(initial?.name ?? "");
  const [enabled, setEnabled] = useState(initial?.enabled ?? true);
  const [minutes, setMinutes] = useState(
    String(Math.max(1, Math.round((initial?.interval_secs ?? 300) / 60))),
  );
  const [command, setCommand] = useState(initial?.command ?? "");
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
  const [test, setTest] = useState<TestResult | null>(null);
  const [testing, setTesting] = useState(false);
  const nameRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    nameRef.current?.focus();
  }, []);

  const applyPreset = (id: string) => {
    const p = AUTOMATION_PRESETS.find((x) => x.id === id);
    if (!p) return;
    if (p.command) setCommand(p.command);
    if (!name.trim() && p.id !== "custom") setName(p.name.split(" — ")[0]);
    setPresetHint(p.hint);
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
    const secs = Math.max(15, Math.round(Number(minutes) * 60) || 300);
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
    });
  };

  const canSave =
    command.trim().length > 0 &&
    (actionKind !== "command" || actionCmd.trim().length > 0) &&
    (actionKind !== "agent" ||
      (agentProject.length > 0 && prompt.trim().length > 0));

  return (
    <div className="modal-overlay" onMouseDown={onClose}>
      <div
        className="modal autoedit"
        onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          if (e.key === "Escape") onClose();
        }}
      >
        <div className="autoedit-row">
          <input
            ref={nameRef}
            className="modal-input"
            placeholder="automation name…"
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
          <select
            className="modal-input autoedit-preset"
            value=""
            onChange={(e) => {
              applyPreset(e.target.value);
              e.target.value = "";
            }}
          >
            <option value="" disabled>
              preset…
            </option>
            {AUTOMATION_PRESETS.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
          </select>
        </div>
        {presetHint && <div className="autoedit-hint">{presetHint}</div>}
        <div className="autoedit-label">poll command — one item per line</div>
        <textarea
          className="modal-input autoedit-cmd"
          placeholder={"shell command whose stdout lists items…"}
          value={command}
          onChange={(e) => setCommand(e.target.value)}
          rows={3}
          spellCheck={false}
        />
        <div className="autoedit-row">
          <label className="autoedit-inline">
            every
            <input
              className="modal-input autoedit-min"
              type="number"
              min={1}
              value={minutes}
              onChange={(e) => setMinutes(e.target.value)}
            />
            min
          </label>
          <button
            type="button"
            className="autoedit-test"
            disabled={testing || !command.trim()}
            onClick={runTest}
          >
            {testing ? "running…" : "test poller"}
          </button>
          <label className="autoedit-inline autoedit-enabled">
            <input
              type="checkbox"
              checked={enabled}
              onChange={(e) => setEnabled(e.target.checked)}
            />
            enabled
          </label>
        </div>
        {test && (
          <div className={`autoedit-testres ${test.ok ? "" : "err"}`}>
            {test.ok
              ? (test.items ?? []).length === 0
                ? "ok — command ran, no items in output"
                : (test.items ?? []).slice(0, 5).map((i) => (
                    <div key={i.id} className="autoedit-testitem">
                      {i.text}
                    </div>
                  ))
              : `error: ${test.error}`}
          </div>
        )}
        <div className="autoedit-label">action for each new item</div>
        <div className="autoedit-actions">
          {ACTION_KINDS.map((k) => (
            <button
              key={k.id}
              type="button"
              className={`autoedit-action ${actionKind === k.id ? "sel" : ""}`}
              onClick={() => setActionKind(k.id)}
              title={k.sub}
            >
              {k.label}
            </button>
          ))}
        </div>
        {actionKind === "command" && (
          <>
            <input
              className="modal-input"
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
                className="modal-input"
                value={agentProject}
                onChange={(e) => setAgentProject(e.target.value)}
              >
                {projects.length === 0 && <option value="">no project</option>}
                {projects.map((p) => (
                  <option key={p.project_id} value={p.project_id}>
                    {p.label ?? p.project_id}
                  </option>
                ))}
              </select>
              <select
                className="modal-input autoedit-kind"
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
              className="modal-input autoedit-cmd"
              placeholder={"prompt template — {id} {text} substituted raw"}
              value={prompt}
              onChange={(e) => setPrompt(e.target.value)}
              rows={3}
              spellCheck={false}
            />
            <div className="autoedit-hint">
              opens a new terminal in the project, starts the agent, then sends
              this prompt
            </div>
          </>
        )}
        {actionKind !== "command" && actionKind !== "agent" && (
          <div className="autoedit-hint">
            first successful poll only records items — actions fire on items
            that appear afterwards
          </div>
        )}
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
          <button
            type="button"
            className="fanout-go"
            disabled={!canSave}
            onClick={submit}
          >
            {initial ? "save" : "create automation"}
          </button>
        </div>
      </div>
    </div>
  );
}
