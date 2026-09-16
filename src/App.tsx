import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  isPermissionGranted,
  requestPermission,
} from "@tauri-apps/plugin-notification";
import { useCallback, useEffect, useRef, useState } from "react";
import { TermGrid } from "./components/TermGrid";
import { AgentPicker } from "./features/AgentPicker";
import { AutomationEditor } from "./features/AutomationEditor";
import { Automations } from "./features/Automations";
import { DiffView } from "./features/DiffView";
import { Fanout, type FanoutRequest } from "./features/Fanout";
import { ImportProject } from "./features/ImportProject";
import { PromptBar, type PromptTarget } from "./features/PromptBar";
import { Session } from "./features/Session";
import { Settings } from "./features/Settings";
import {
  type Automation,
  type AutomationInput,
  automations,
} from "./shared/automations";
import {
  type AgentStatus,
  type LazedEvent,
  type Snapshot,
  type TerminalInfo,
  lazed,
  subscribeEvents,
} from "./shared/lazed";
import { notificationsEnabled } from "./shared/settings";
import { InboxPanel } from "./widgets/Inbox";
import { Rail, type RailView } from "./widgets/Rail";
import { Sidebar } from "./widgets/Sidebar";

// daemon event names that mean "the model changed — refetch the snapshot"
const REFRESH_EVENTS = new Set([
  "project.created",
  "project.closed",
  "project.focused",
  "group.created",
  "group.updated",
  "group.removed",
  "terminal.created",
  "terminal.closed",
  "events.reconnect",
]);

function worst(statuses: (AgentStatus | undefined)[]): AgentStatus {
  for (const s of ["blocked", "working", "done", "idle"] as const) {
    if (statuses.includes(s)) return s;
  }
  return "unknown";
}

interface NotifyOpts {
  title: string;
  body: string;
  sound?: string;
  termId: string;
  projectId?: string;
  /** don't alert when the user is already looking at this terminal */
  skipIfFocused?: boolean;
}

/** macOS notification for an agent transition — the backend sends it via
 * notify-rust so a body click can round-trip as a `notification.jump` event
 * (the notification plugin's onAction is mobile-only). */
async function notifyAgent(o: NotifyOpts) {
  if (!notificationsEnabled()) return;
  try {
    let granted = await isPermissionGranted();
    if (!granted) granted = (await requestPermission()) === "granted";
    if (!granted) return;
    if (o.skipIfFocused && (await getCurrentWindow().isFocused())) return;
    await invoke("notify_agent", {
      termId: o.termId,
      projectId: o.projectId,
      title: o.title,
      body: o.body,
      sound: o.sound,
    });
  } catch {
    // notifications unavailable — ignore
  }
}

function basename(p?: string) {
  if (!p) return "";
  const parts = p.replace(/\/$/, "").split("/");
  return parts[parts.length - 1] || p;
}

/** First prompt for a project's orchestrator agent — points at the lazed
 * skill and describes the fan-out loop over worktree terminals. */
function orchestratorPreamble(projectId: string, repoRoot: string): string {
  return [
    "You orchestrate this project. The `lazed` skill describes how to orchestrate other terminals — read it if it is not already loaded.",
    `Repo root: ${repoRoot}. Project id: ${projectId}. Your own terminal id is in $LAZED_TERM.`,
    "Do all implementation work in worktree terminals — use this terminal only to coordinate (status, merge, report).",
    `For each task: \`lazed api worktree.create '{"project_id":"${projectId}","branch":"<slug>"}'\` → \`lazed api agent.start '{"term_id":"<t>","kind":"claude"}'\` → \`lazed api agent.prompt '{"term_id":"<t>","text":"<task>"}'\` → \`lazed api agent.wait '{"term_id":"<t>","until":["idle","blocked"]}'\` → verify via \`lazed api terminal.read '{"term_id":"<t>","lines":50}'\` and report branch + commits.`,
    "If a spawned agent sits on a trust/permission dialog, `terminal.read` it and send the exact key it asks for via `terminal.input`.",
  ].join("\n");
}

export function App() {
  const [snap, setSnap] = useState<Snapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [focusedTermId, setFocusedTermId] = useState<string | null>(null);
  const [showPicker, setShowPicker] = useState(false);
  const [showPrompt, setShowPrompt] = useState(false);
  const [showFanout, setShowFanout] = useState(false);
  const [diffTerm, setDiffTerm] = useState<TerminalInfo | null>(null);
  const [inboxOpen, setInboxOpen] = useState(false);
  const [dismissed, setDismissed] = useState<ReadonlySet<string>>(new Set());
  const [showSettings, setShowSettings] = useState(false);
  const [showSession, setShowSession] = useState(false);
  const [showAutos, setShowAutos] = useState(false);
  const [showImport, setShowImport] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  // left-rail space switch — projects is currently the only panel view;
  // automations and session are full-screen overlays
  const [railView, setRailView] = useState<RailView>("projects");
  const [autos, setAutos] = useState<Automation[]>([]);
  const [autoEditor, setAutoEditor] = useState<{
    auto?: Automation;
  } | null>(null);
  const refreshTimer = useRef<number | null>(null);
  const noticeTimer = useRef<number | null>(null);
  // event callbacks run outside render — mirror the bits they need
  const snapRef = useRef<Snapshot | null>(null);
  const focusRef = useRef<string | null>(null);

  const selectRail = useCallback((v: RailView) => {
    setRailView(v);
    localStorage.setItem("lazed-rail", v);
  }, []);

  const loadAutos = useCallback(() => {
    automations
      .list()
      .then(setAutos)
      .catch(() => {});
  }, []);

  const flash = useCallback((msg: string) => {
    setNotice(msg);
    if (noticeTimer.current !== null) clearTimeout(noticeTimer.current);
    noticeTimer.current = window.setTimeout(() => setNotice(null), 5000);
  }, []);

  const refresh = useCallback(() => {
    lazed
      .snapshot()
      .then(setSnap)
      .catch((e) => setError(String(e)));
  }, []);

  const scheduleRefresh = useCallback(() => {
    if (refreshTimer.current !== null) return;
    refreshTimer.current = window.setTimeout(() => {
      refreshTimer.current = null;
      refresh();
    }, 40);
  }, [refresh]);

  useEffect(() => {
    lazed
      .bootstrap()
      .then((res) => {
        if (res.snapshot) setSnap(res.snapshot);
        else refresh();
      })
      .catch((e) => setError(String(e)));
    loadAutos();
    subscribeEvents((ev: LazedEvent) => {
      const name = ev.event ?? ev.type ?? "";
      if (!name) return;
      if (name === "automation.updated") {
        loadAutos();
        return;
      }
      if (name === "agent.status") {
        const d = ev.data ?? {};
        const termId = d.term_id as string | undefined;
        const status = d.agent_status as AgentStatus | undefined;
        if (termId && status) {
          // a terminal that goes back to work re-earns its next inbox entry
          if (status === "working" || status === "idle") {
            setDismissed((prev) => {
              if (!prev.has(termId)) return prev;
              const next = new Set(prev);
              next.delete(termId);
              return next;
            });
          }
          if (status === "blocked" || status === "done") {
            const cur = snapRef.current;
            const term = cur?.terminals.find((x) => x.term_id === termId);
            const proj = cur?.projects.find((p) =>
              p.terminals.includes(termId),
            );
            const who = (d.agent as string) ?? term?.agent_kind ?? termId;
            const where = [
              proj?.label ?? basename(proj?.repo_root),
              term?.label ?? (term?.cwd ? basename(term.cwd) : undefined),
            ]
              .filter(Boolean)
              .join(" · ");
            notifyAgent({
              title:
                status === "done" ? `${who} finished` : `${who} needs input`,
              body: where || termId,
              sound: status === "done" ? "Glass" : "Ping",
              termId,
              projectId: proj?.project_id,
              skipIfFocused: focusRef.current === termId,
            });
          }
          setSnap((prev) =>
            prev
              ? {
                  ...prev,
                  terminals: prev.terminals.map((t) =>
                    t.term_id === termId
                      ? {
                          ...t,
                          agent_status: status,
                          agent_kind: (d.agent as string) ?? t.agent_kind,
                        }
                      : t,
                  ),
                }
              : prev,
          );
        }
        return;
      }
      if (REFRESH_EVENTS.has(name)) scheduleRefresh();
    }).catch((e) => setError(String(e)));
  }, [refresh, scheduleRefresh, loadAutos]);

  const saveAutomation = useCallback(
    (input: AutomationInput) => {
      automations
        .save(input)
        .then(() => {
          setAutoEditor(null);
          loadAutos();
        })
        .catch((e) => setError(String(e)));
    },
    [loadAutos],
  );

  const projects = snap?.projects ?? [];
  const termsById = new Map((snap?.terminals ?? []).map((t) => [t.term_id, t]));

  const focusedProjectId = snap?.focused_project_id ?? projects[0]?.project_id;
  const focusedProject = projects.find(
    (p) => p.project_id === focusedProjectId,
  );
  /** The focused project's terminals in display order — the root-checkout
   * terminals first, then worktrees (project.terminals order). */
  const projectTerms = (focusedProject?.terminals ?? [])
    .map((id) => termsById.get(id))
    .filter((t): t is TerminalInfo => Boolean(t));

  const effectiveFocusedTerm =
    focusedTermId && termsById.has(focusedTermId)
      ? focusedTermId
      : (projectTerms[0]?.term_id ?? null);

  // keep the event-callback mirrors current
  useEffect(() => {
    snapRef.current = snap;
  }, [snap]);
  useEffect(() => {
    focusRef.current = effectiveFocusedTerm;
  }, [effectiveFocusedTerm]);

  const focusProject = useCallback((id: string) => {
    lazed.projectFocus(id).catch((e) => setError(String(e)));
    setFocusedTermId(null);
  }, []);

  const jumpToTerm = useCallback((termId: string, projectId: string) => {
    lazed.projectFocus(projectId).catch(() => {});
    setFocusedTermId(termId);
    setInboxOpen(false);
  }, []);

  // notification click → raise the window and jump to that terminal
  useEffect(() => {
    let off: (() => void) | undefined;
    listen<{ term_id?: string; project_id?: string }>(
      "notification.jump",
      (e) => {
        const termId = e.payload.term_id;
        if (!termId) return;
        const projectId =
          e.payload.project_id ??
          snapRef.current?.projects.find((p) => p.terminals.includes(termId))
            ?.project_id;
        const win = getCurrentWindow();
        win
          .show()
          .then(() => win.unminimize())
          .then(() => win.setFocus())
          .catch(() => {});
        if (projectId) jumpToTerm(termId, projectId);
        else setFocusedTermId(termId);
      },
    )
      .then((unlisten) => {
        off = unlisten;
      })
      .catch(() => {});
    return () => off?.();
  }, [jumpToTerm]);

  const closeTerm = useCallback((t: TerminalInfo) => {
    setFocusedTermId((cur) => (cur === t.term_id ? null : cur));
    if (t.kind === "worktree") {
      lazed.worktreeRemove(t.term_id, false).catch((e) => setError(String(e)));
    } else {
      lazed.termClose(t.term_id).catch((e) => setError(String(e)));
    }
  }, []);

  const newTerminal = useCallback(() => {
    if (!focusedProjectId) return;
    lazed
      .termCreate(focusedProjectId)
      .then((t) => setFocusedTermId(t.term_id))
      .catch((e) => setError(String(e)));
  }, [focusedProjectId]);

  const newProject = useCallback(
    async (cwd?: string, label?: string) => {
      setShowImport(false);
      try {
        if (cwd) {
          // one project per repo: focus instead of duplicating
          const repo = await lazed.resolveRepo(cwd).catch(() => null);
          if (repo?.repo_key) {
            const live = await lazed.snapshot().catch(() => null);
            const existing = live?.projects.find(
              (p) => p.repo_key === repo.repo_key,
            );
            if (existing) {
              focusProject(existing.project_id);
              flash(
                `already imported — focused ${existing.label ?? existing.project_id}`,
              );
              return;
            }
          }
        }
        const res = await lazed.projectCreate(cwd ?? "", label);
        const pid = res.project?.project_id;
        if (pid) await lazed.projectFocus(pid);
        const tid = res.terminal?.term_id;
        if (tid) setFocusedTermId(tid);
      } catch (e) {
        setError(String(e));
      }
    },
    [focusProject, flash],
  );

  const spawnOrchestrator = useCallback(
    async (projectId: string) => {
      const live = await lazed.snapshot().catch(() => null);
      const p = live?.projects.find((x) => x.project_id === projectId);
      if (!p) return;
      const terms = (live?.terminals ?? []).filter((t) =>
        p.terminals.includes(t.term_id),
      );
      const root = terms.find((t) => t.kind !== "worktree") ?? terms[0];
      if (!root) {
        setError("orchestrator: project has no terminal");
        return;
      }
      lazed.projectFocus(projectId).catch(() => {});
      setFocusedTermId(root.term_id);
      // agent_start errors when the terminal already runs an agent — fine,
      // the readiness poll below just re-arms the preamble on the live one
      await lazed.agentStart(root.term_id, "claude").catch(() => {});
      const deadline = Date.now() + 30000;
      while (Date.now() < deadline) {
        const a = await lazed.agentGet(root.term_id).catch(() => null);
        if (
          a?.agent_status === "idle" ||
          a?.agent_status === "working" ||
          a?.agent_status === "done"
        ) {
          await lazed
            .agentPrompt(
              root.term_id,
              orchestratorPreamble(projectId, p.repo_root),
            )
            .then(() => flash("orchestrator started"))
            .catch((e) => setError(String(e)));
          return;
        }
        // blocked usually means a trust dialog — leave it to the user,
        // approval flips the status and the poll continues
        await new Promise((r) => setTimeout(r, 600));
      }
      setError(
        "orchestrator: agent not ready — approve the dialog in the terminal, then retry",
      );
    },
    [flash],
  );

  const startAgent = useCallback(
    (kind: string) => {
      const target = effectiveFocusedTerm;
      setShowPicker(false);
      if (!target) return;
      lazed.agentStart(target, kind).catch((e) => setError(String(e)));
    },
    [effectiveFocusedTerm],
  );

  const submitPrompt = useCallback(
    (text: string, target: PromptTarget) => {
      const agentTerms = projectTerms.filter((t) => t.agent_kind);
      const run = (t: TerminalInfo) =>
        (t.agent_kind
          ? lazed.agentPrompt(t.term_id, text)
          : lazed.termSend(t.term_id, `${text}\n`)
        ).catch((e) => setError(String(e)));
      if (target.kind === "focused") {
        const t = termsById.get(target.termId);
        if (t) run(t);
      } else if (target.kind === "agents") {
        for (const t of agentTerms) run(t);
      } else {
        for (const t of projectTerms) run(t);
      }
    },
    [projectTerms, termsById],
  );

  const doFanout = useCallback(
    async (req: FanoutRequest) => {
      setShowFanout(false);
      const repo = req.repo.trim() || focusedProject?.repo_root || "";
      if (!repo) {
        setError("fan-out: repo path is required");
        return;
      }
      for (const kind of req.kinds) {
        const branch = `${req.prefix}-${kind}`;
        try {
          const res = await lazed.worktreeCreate(
            focusedProjectId ?? "",
            branch,
            req.base,
            branch,
          );
          const termId = res.terminal?.term_id;
          if (!termId) continue;
          // let the fresh shell initialize before launching the agent
          await new Promise((r) => setTimeout(r, 1500));
          await lazed.agentStart(termId, kind);
          if (req.prompt) {
            // wait until detection leaves "unknown", then a short settle
            // for TUI paint. Bounded: a missed detection degrades to the
            // old fixed delay.
            const deadline = Date.now() + 4000;
            while (Date.now() < deadline) {
              try {
                const a = await lazed.agentGet(termId);
                if (a.agent_status && a.agent_status !== "unknown") break;
              } catch {
                // agent not detected yet
              }
              await new Promise((r) => setTimeout(r, 250));
            }
            await new Promise((r) => setTimeout(r, 1000));
            await lazed.agentPrompt(termId, req.prompt);
          }
        } catch (e) {
          setError(String(e));
        }
      }
      void repo;
    },
    [focusedProjectId, focusedProject],
  );

  // app-level shortcuts — registered in the capture phase so chords that
  // the terminal would otherwise consume are intercepted before the IME
  // overlay input sees them
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey || e.altKey)) return;
      if (showSettings) return;
      const projectIds = projects.map((p) => p.project_id);
      const termIds = projectTerms.map((t) => t.term_id);
      const step = (list: string[], cur: string | undefined, d: number) => {
        const i = Math.max(0, list.indexOf(cur ?? ""));
        return list[(i + d + list.length) % list.length];
      };
      // physical digit position — Option transforms e.key (⌥1 → "¡")
      const digit = /^Digit([1-9])$/.exec(e.code)?.[1];
      // full-screen overlays swallow all chords; their own digit toggles off
      if (showSession || showAutos) {
        const close =
          (showAutos && digit === "2") || (showSession && digit === "3");
        if (e.metaKey && e.shiftKey && close) {
          e.preventDefault();
          e.stopPropagation();
          if (showAutos) setShowAutos(false);
          else setShowSession(false);
        }
        return;
      }
      if (e.metaKey && !e.shiftKey && e.key === "d") {
        e.preventDefault();
        e.stopPropagation();
        newTerminal();
      } else if (e.metaKey && e.shiftKey && (e.key === "d" || e.key === "D")) {
        e.preventDefault();
        e.stopPropagation();
        newTerminal();
      } else if (e.metaKey && !e.shiftKey && e.key === "w") {
        e.preventDefault();
        e.stopPropagation();
        const t = effectiveFocusedTerm
          ? termsById.get(effectiveFocusedTerm)
          : undefined;
        if (t) closeTerm(t);
      } else if (e.metaKey && !e.shiftKey && e.key === "t") {
        e.preventDefault();
        e.stopPropagation();
        newTerminal();
      } else if (e.metaKey && e.shiftKey && (e.key === "n" || e.key === "N")) {
        e.preventDefault();
        e.stopPropagation();
        setShowImport(true);
      } else if (e.metaKey && e.shiftKey && e.key === "]") {
        e.preventDefault();
        e.stopPropagation();
        const next = step(projectIds, focusedProjectId, 1);
        if (next) focusProject(next);
      } else if (e.metaKey && e.shiftKey && e.key === "[") {
        e.preventDefault();
        e.stopPropagation();
        const prev = step(projectIds, focusedProjectId, -1);
        if (prev) focusProject(prev);
      } else if (e.metaKey && !e.shiftKey && e.key === "]") {
        e.preventDefault();
        e.stopPropagation();
        const next = step(termIds, effectiveFocusedTerm ?? undefined, 1);
        if (next) setFocusedTermId(next);
      } else if (e.metaKey && !e.shiftKey && e.key === "[") {
        e.preventDefault();
        e.stopPropagation();
        const prev = step(termIds, effectiveFocusedTerm ?? undefined, -1);
        if (prev) setFocusedTermId(prev);
      } else if (e.metaKey && !e.shiftKey && e.key === "k") {
        e.preventDefault();
        e.stopPropagation();
        setShowPrompt(true);
      } else if (e.metaKey && e.shiftKey && (e.key === "a" || e.key === "A")) {
        e.preventDefault();
        e.stopPropagation();
        setShowPicker(true);
      } else if (e.metaKey && e.shiftKey && (e.key === "i" || e.key === "I")) {
        e.preventDefault();
        e.stopPropagation();
        setInboxOpen((o) => !o);
      } else if (e.metaKey && e.shiftKey && (e.key === "f" || e.key === "F")) {
        e.preventDefault();
        e.stopPropagation();
        setShowFanout(true);
      } else if (e.metaKey && !e.shiftKey && e.key === ",") {
        e.preventDefault();
        e.stopPropagation();
        setShowSettings(true);
      } else if (e.metaKey && e.shiftKey && digit === "1") {
        e.preventDefault();
        e.stopPropagation();
        selectRail("projects");
      } else if (e.metaKey && e.shiftKey && digit === "2") {
        e.preventDefault();
        e.stopPropagation();
        setShowAutos(true);
      } else if (e.metaKey && e.shiftKey && digit === "3") {
        e.preventDefault();
        e.stopPropagation();
        setShowSession(true);
      } else if (e.metaKey && !e.shiftKey && !e.ctrlKey && !e.altKey && digit) {
        // ⌘1-9 — focus project by index
        e.preventDefault();
        e.stopPropagation();
        const p = projects[Number(digit) - 1];
        if (p) focusProject(p.project_id);
      } else if (!e.metaKey && !e.ctrlKey && e.altKey && !e.shiftKey && digit) {
        // ⌥1-9 — focus terminal N within the focused project
        e.preventDefault();
        e.stopPropagation();
        const tid = termIds[Number(digit) - 1];
        if (tid) setFocusedTermId(tid);
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [
    projects,
    projectTerms,
    termsById,
    focusedProjectId,
    effectiveFocusedTerm,
    showSettings,
    showSession,
    showAutos,
    newTerminal,
    closeTerm,
    focusProject,
    selectRail,
  ]);

  const allTerms = [...termsById.values()];
  const inboxItems = allTerms
    .filter(
      (t) =>
        (t.agent_status === "blocked" || t.agent_status === "done") &&
        !dismissed.has(t.term_id),
    )
    .sort((a, b) =>
      a.agent_status === b.agent_status
        ? 0
        : a.agent_status === "blocked"
          ? -1
          : 1,
    )
    .map((term) => ({
      term,
      project:
        projects.find((p) => p.terminals.includes(term.term_id))?.label ??
        basename(
          projects.find((p) => p.terminals.includes(term.term_id))?.repo_root,
        ),
    }));
  const agentCount = projectTerms.filter((t) => t.agent_kind).length;

  // dock badge = undismissed attention count (blocked + finished agents)
  useEffect(() => {
    getCurrentWindow()
      .setBadgeCount(inboxItems.length || undefined)
      .catch(() => {});
  }, [inboxItems.length]);

  return (
    <div className="app">
      <div
        className="titlebar"
        data-tauri-drag-region
        onMouseDown={(e) => {
          if (e.button === 0 && e.target === e.currentTarget) {
            getCurrentWindow()
              .startDragging()
              .catch(() => {});
          }
        }}
      >
        <span className="title">LAZED</span>
        <span className="status">
          {error
            ? `error: ${error}`
            : notice
              ? notice
              : snap
                ? ""
                : "connecting…"}
        </span>
      </div>
      {inboxOpen && (
        <InboxPanel
          items={inboxItems}
          onJump={(t) => {
            const p = projects.find((x) => x.terminals.includes(t.term_id));
            if (p) jumpToTerm(t.term_id, p.project_id);
          }}
          onDismiss={(id) => setDismissed((prev) => new Set(prev).add(id))}
          onDismissAll={() =>
            setDismissed(
              (prev) =>
                new Set([...prev, ...inboxItems.map((i) => i.term.term_id)]),
            )
          }
          onClose={() => setInboxOpen(false)}
        />
      )}
      {showPicker && (
        <AgentPicker onPick={startAgent} onClose={() => setShowPicker(false)} />
      )}
      {showPrompt && (
        <PromptBar
          focusedTerm={effectiveFocusedTerm}
          agentCount={agentCount}
          termCount={projectTerms.length}
          onSubmit={submitPrompt}
          onClose={() => setShowPrompt(false)}
        />
      )}
      {showFanout && (
        <Fanout
          defaultRepo={focusedProject?.repo_root ?? ""}
          onSubmit={doFanout}
          onClose={() => setShowFanout(false)}
        />
      )}
      {showSettings && <Settings onClose={() => setShowSettings(false)} />}
      {showSession && (
        <Session
          snap={snap}
          focusedTermId={effectiveFocusedTerm}
          onJumpTerm={jumpToTerm}
          onFocusProject={focusProject}
          onClose={() => setShowSession(false)}
        />
      )}
      {showAutos && (
        <Automations
          autos={autos}
          onNew={() => setAutoEditor({})}
          onEdit={(a) => setAutoEditor({ auto: a })}
          onChanged={loadAutos}
          editorOpen={autoEditor != null}
          onClose={() => setShowAutos(false)}
        />
      )}
      {showImport && (
        <ImportProject
          onImport={newProject}
          onClose={() => setShowImport(false)}
        />
      )}
      {autoEditor && (
        <AutomationEditor
          initial={autoEditor.auto}
          projects={projects}
          onSave={saveAutomation}
          onDeleteSeen={(id) =>
            automations
              .resetSeen(id)
              .then(loadAutos)
              .catch((e) => setError(String(e)))
          }
          onClose={() => setAutoEditor(null)}
        />
      )}
      {diffTerm &&
        (() => {
          const p = projects.find((x) =>
            x.terminals.includes(diffTerm.term_id),
          );
          return (
            <DiffView
              checkout={diffTerm.cwd}
              repoRoot={p?.repo_root}
              label={diffTerm.label}
              termId={diffTerm.term_id}
              agentTermId={diffTerm.agent_kind ? diffTerm.term_id : undefined}
              onClose={() => setDiffTerm(null)}
              onJump={() =>
                p ? jumpToTerm(diffTerm.term_id, p.project_id) : undefined
              }
            />
          );
        })()}
      <div className="body">
        <Rail
          active={railView}
          onSelect={selectRail}
          onAutomations={() => setShowAutos(true)}
          onSession={() => setShowSession(true)}
          onSettings={() => setShowSettings(true)}
          automationAlert={autos.some((a) => a.last_error)}
        />
        <Sidebar
          snap={snap}
          focusedTermId={effectiveFocusedTerm}
          onFocusProject={focusProject}
          onJumpTerm={jumpToTerm}
          onNewProject={() => setShowImport(true)}
          onNewGroup={() =>
            lazed
              .groupCreate()
              .then((g) => g?.group_id ?? null)
              .catch((e) => {
                setError(String(e));
                return null;
              })
          }
          onCloseProject={(id) =>
            lazed.projectClose(id).catch((e) => setError(String(e)))
          }
          onRenameProject={(id, label) =>
            lazed.projectRename(id, label).catch((e) => setError(String(e)))
          }
          onRenameGroup={(id, label) =>
            lazed.groupRename(id, label).catch((e) => setError(String(e)))
          }
          onRemoveGroup={(id) =>
            lazed.groupRemove(id).catch((e) => setError(String(e)))
          }
          onAssignProject={(pid, gid) =>
            lazed.groupAssign(pid, gid).catch((e) => setError(String(e)))
          }
          onCloseTerm={closeTerm}
          onDiff={setDiffTerm}
          onOrchestrate={spawnOrchestrator}
          rollup={worst}
        />
        <div className="main">
          {projectTerms.length > 0 ? (
            <TermGrid
              terms={projectTerms}
              focusedTerm={effectiveFocusedTerm}
              onFocusTerm={setFocusedTermId}
              onCloseTerm={(id) => {
                const t = termsById.get(id);
                if (t) closeTerm(t);
              }}
            />
          ) : (
            <div className="empty">
              {error
                ? `failed: ${error}`
                : snap
                  ? projects.length === 0
                    ? "import a project to begin — ⇧⌘N"
                    : "no terminals in this project — ⌘D to add one"
                  : "connecting to lazed…"}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
