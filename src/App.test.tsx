// @vitest-environment jsdom
import { act, useEffect, useRef } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, it, vi } from "vitest";
import type { LazedEvent, Snapshot, TerminalInfo } from "./shared/lazed";

const backend = vi.hoisted(() => ({
  invoke: vi.fn(),
  event: (_event: LazedEvent) => {},
}));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: backend.invoke,
  Channel: class {},
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ setBadgeCount: async () => {} }),
}));
vi.mock("./shared/lazed", async (original) => ({
  ...(await original<typeof import("./shared/lazed")>()),
  subscribeEvents: async (callback: typeof backend.event) => {
    backend.event = callback;
    return () => {};
  },
}));
// Preserve the DOM focus -> selection feedback used by the real terminal.
vi.mock("./components/TermView", () => ({
  TermView: ({
    term,
    focused,
    onFocus,
  }: {
    term: TerminalInfo;
    focused: boolean;
    onFocus: (id: string) => void;
  }) => {
    const input = useRef<HTMLInputElement>(null);
    useEffect(() => {
      if (focused) input.current?.focus();
    }, [focused]);
    return (
      <input
        ref={input}
        data-term={term.term_id}
        onFocus={() => onFocus(term.term_id)}
      />
    );
  },
}));

import { App } from "./App";

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;
const host = document.createElement("div");
document.body.append(host);
let root: ReturnType<typeof createRoot>;

afterEach(() => {
  act(() => root.unmount());
  vi.useRealTimers();
  vi.clearAllMocks();
});

it.each([false, true])(
  "focuses the new pane with event-first=%s",
  async (eventFirst) => {
    vi.useFakeTimers();
    const terminal = (id: string): TerminalInfo => ({
      term_id: id,
      project_id: "p1",
      workspace_id: "w1",
      tab_id: "tab1",
      cwd: "/tmp",
    });
    const snapshot = (ids: string[]): Snapshot => ({
      focused_project_id: "p1",
      projects: [
        {
          project_id: "p1",
          repo_root: "/tmp",
          repo_key: "demo",
          workspaces: ["w1"],
        },
      ],
      workspaces: [
        {
          workspace_id: "w1",
          project_id: "p1",
          path: "/tmp",
          is_main: true,
          tabs: ["tab1"],
        },
      ],
      tabs: [{ tab_id: "tab1", workspace_id: "w1", panes: ids }],
      terminals: ids.map(terminal),
    });
    let current = snapshot(["t1", "t2"]);
    let created!: (t: TerminalInfo) => void;
    backend.invoke.mockImplementation(async (command: string) => {
      if (command === "bootstrap") return { snapshot: current };
      if (command === "session_snapshot") return current;
      if (command === "install_status") return { ok: true };
      if (command === "inbox_list") return { items: [] };
      if (command === "automation_list") return { automations: [] };
      if (command === "term_create")
        return new Promise<TerminalInfo>((resolve) => {
          created = resolve;
        });
      return [];
    });
    root = createRoot(host);
    await act(async () => root.render(<App />));
    act(() =>
      host.querySelector<HTMLInputElement>('[data-term="t2"]')?.focus(),
    );
    act(() =>
      window.dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "d",
          code: "KeyD",
          metaKey: true,
          bubbles: true,
        }),
      ),
    );
    expect(backend.invoke).toHaveBeenCalledWith("term_create", {
      tabId: "tab1",
    });
    current = snapshot(["t1", "t2", "t3"]);
    if (eventFirst) {
      await act(async () => {
        backend.event({ event: "terminal.created" });
        await vi.advanceTimersByTimeAsync(40);
      });
    }
    await act(async () => created(terminal("t3")));
    if (!eventFirst) {
      await act(async () => {
        backend.event({ event: "terminal.created" });
        await vi.advanceTimersByTimeAsync(40);
      });
    }
    expect(document.activeElement?.getAttribute("data-term")).toBe("t3");
  },
);

it.each([false, true])(
  "creates a terminal from an empty workspace with existing tab=%s",
  async (hasTab) => {
    const terminal: TerminalInfo = {
      term_id: "t1",
      project_id: "p1",
      workspace_id: "w1",
      tab_id: "tab1",
      cwd: "/tmp",
    };
    let current: Snapshot = {
      focused_project_id: "p1",
      projects: [
        {
          project_id: "p1",
          repo_root: "/tmp",
          repo_key: "demo",
          workspaces: ["w1"],
        },
      ],
      workspaces: [
        {
          workspace_id: "w1",
          project_id: "p1",
          path: "/tmp",
          is_main: true,
          tabs: hasTab ? ["tab1"] : [],
        },
      ],
      tabs: hasTab ? [{ tab_id: "tab1", workspace_id: "w1", panes: [] }] : [],
      terminals: [],
    };
    let fail = true;
    backend.invoke.mockImplementation(async (command: string) => {
      if (command === "bootstrap") return { snapshot: current };
      if (command === "session_snapshot") return current;
      if (command === "install_status") return { ok: true };
      if (command === "inbox_list") return { items: [] };
      if (command === "automation_list") return { automations: [] };
      if (command === "tab_create" || command === "term_create") {
        if (fail) throw new Error("shell unavailable");
        current = {
          ...current,
          workspaces: current.workspaces?.map((ws) => ({
            ...ws,
            tabs: ["tab1"],
          })),
          tabs: [{ tab_id: "tab1", workspace_id: "w1", panes: ["t1"] }],
          terminals: [terminal],
        };
        return command === "tab_create"
          ? { tab_id: "tab1", terminal }
          : terminal;
      }
      return [];
    });
    root = createRoot(host);
    await act(async () => root.render(<App />));
    const create = () =>
      [...host.querySelectorAll("button")].find(
        (b) => b.textContent === "new terminal (⌘D)",
      );
    expect(create()).toBeDefined();
    await act(async () => create()?.click());
    expect(host.querySelector('[role="alert"]')?.textContent).toContain(
      "shell unavailable",
    );
    fail = false;
    await act(async () => create()?.click());
    expect(backend.invoke).toHaveBeenCalledWith(
      hasTab ? "term_create" : "tab_create",
      hasTab
        ? { tabId: "tab1" }
        : { workspaceId: "w1", label: undefined, command: undefined },
    );
    expect(document.activeElement?.getAttribute("data-term")).toBe("t1");
    expect(create()).toBeUndefined();
  },
);
