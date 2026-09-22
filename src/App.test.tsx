// @vitest-environment jsdom
import { act, useEffect, useRef } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, it, vi } from "vitest";
import type { LazedEvent, PaneInfo, Snapshot } from "./shared/lazed";

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
    term: PaneInfo;
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
        data-term={term.pane_id}
        onFocus={() => onFocus(term.pane_id)}
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
    const terminal = (id: string): PaneInfo => ({
      pane_id: id,
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
      panes: ids.map(terminal),
    });
    let current = snapshot(["t1", "t2"]);
    let created!: (r: { pane: PaneInfo }) => void;
    backend.invoke.mockImplementation(async (command: string) => {
      if (command === "bootstrap") return { snapshot: current };
      if (command === "session_snapshot") return current;
      if (command === "install_status") return { ok: true };
      if (command === "inbox_list") return { items: [] };
      if (command === "automation_list") return { automations: [] };
      if (command === "pane_create")
        return new Promise<{ pane: PaneInfo }>((resolve) => {
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
    expect(backend.invoke).toHaveBeenCalledWith("pane_create", {
      targetPaneId: "t2",
    });
    current = snapshot(["t1", "t2", "t3"]);
    if (eventFirst) {
      await act(async () => {
        backend.event({ event: "herdr.pane_created" });
        await vi.advanceTimersByTimeAsync(40);
      });
    }
    await act(async () => created({ pane: terminal("t3") }));
    if (!eventFirst) {
      await act(async () => {
        backend.event({ event: "herdr.pane_created" });
        await vi.advanceTimersByTimeAsync(40);
      });
    }
    expect(document.activeElement?.getAttribute("data-term")).toBe("t3");
  },
);

it.each([false, true])(
  "creates a terminal from an empty workspace with existing tab=%s",
  async (hasTab) => {
    const terminal: PaneInfo = {
      pane_id: "t1",
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
      panes: [],
    };
    let fail = true;
    backend.invoke.mockImplementation(async (command: string) => {
      if (command === "bootstrap") return { snapshot: current };
      if (command === "session_snapshot") return current;
      if (command === "install_status") return { ok: true };
      if (command === "inbox_list") return { items: [] };
      if (command === "automation_list") return { automations: [] };
      if (command === "tab_create" || command === "pane_create") {
        if (fail) throw new Error("shell unavailable");
        current = {
          ...current,
          workspaces: current.workspaces?.map((ws) => ({
            ...ws,
            tabs: ["tab1"],
          })),
          tabs: [{ tab_id: "tab1", workspace_id: "w1", panes: ["t1"] }],
          panes: [terminal],
        };
        return command === "tab_create"
          ? {
              tab: { tab_id: "tab1", workspace_id: "w1", panes: [] },
              root_pane: terminal,
            }
          : { pane: terminal };
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
    // an empty tab has no pane to split next to — both cases open a tab
    expect(backend.invoke).toHaveBeenCalledWith("tab_create", {
      workspaceId: "w1",
      label: undefined,
    });
    expect(document.activeElement?.getAttribute("data-term")).toBe("t1");
    expect(create()).toBeUndefined();
  },
);

it("⌘N creates a worktree for the selected project", async () => {
  const snapshot: Snapshot = {
    focused_project_id: "p1",
    projects: [
      {
        project_id: "p1",
        repo_root: "/tmp/demo",
        repo_key: "demo",
        workspaces: ["w1"],
      },
    ],
    workspaces: [
      {
        workspace_id: "w1",
        project_id: "p1",
        path: "/tmp/demo",
        is_main: true,
        tabs: ["tab1"],
      },
    ],
    tabs: [{ tab_id: "tab1", workspace_id: "w1", panes: ["t1"] }],
    panes: [
      {
        pane_id: "t1",
        workspace_id: "w1",
        tab_id: "tab1",
        cwd: "/tmp/demo",
      },
    ],
  };
  backend.invoke.mockImplementation(async (command: string) => {
    if (command === "bootstrap") return { snapshot };
    if (command === "session_snapshot") return snapshot;
    if (command === "install_status") return { ok: true };
    if (command === "inbox_list") return { items: [] };
    if (command === "automation_list") return { automations: [] };
    if (command === "workspace_create")
      return {
        checkout_path: "/tmp/wt",
        branch: "feature/x",
        project_id: "p1",
        workspace_id: "w2",
        tab_id: "tab2",
        pane_id: "t2",
      };
    return [];
  });
  root = createRoot(host);
  await act(async () => root.render(<App />));
  act(() =>
    window.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "n",
        code: "KeyN",
        metaKey: true,
        bubbles: true,
      }),
    ),
  );

  const branch = host.querySelector<HTMLInputElement>(".newwt-input");
  expect(branch).toBeTruthy();
  const setter = Object.getOwnPropertyDescriptor(
    HTMLInputElement.prototype,
    "value",
  )?.set;
  await act(async () => {
    setter?.call(branch, "feature/x");
    branch?.dispatchEvent(new Event("input", { bubbles: true }));
  });
  const create = [...host.querySelectorAll<HTMLButtonElement>("button")].find(
    (b) => b.textContent === "Create worktree",
  );
  await act(async () => create?.click());

  expect(backend.invoke).toHaveBeenCalledWith("workspace_create", {
    projectId: "p1",
    branch: "feature/x",
    base: undefined,
    label: undefined,
  });
  // the sheet closes once the daemon accepted the worktree
  expect(host.querySelector(".newwt-input")).toBeNull();
});
