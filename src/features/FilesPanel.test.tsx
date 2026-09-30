// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, it, vi } from "vitest";

const backend = vi.hoisted(() => ({
  invoke: vi.fn(),
  event: (_event: { payload: { watch_id: string; error?: string } }) => {},
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (_name, callback) => {
    backend.event = callback;
    return () => {};
  }),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: backend.invoke }));

import type { FsTree } from "../shared/lazed";
import { FilesPanel } from "./FilesPanel";

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;
const host = document.createElement("div");
document.body.append(host);
let root: ReturnType<typeof createRoot>;

const TREE: FsTree = {
  root: "/repo",
  git: true,
  entries: [
    { path: "src", kind: "dir", git: "M" },
    { path: "src/App.tsx", kind: "file", git: "M" },
    { path: "README.md", kind: "file", git: null },
    { path: "new.txt", kind: "file", git: "U" },
    { path: "dist", kind: "dir", git: "!", collapsed: true },
  ],
};

function flush() {
  return act(async () => {
    await Promise.resolve();
  });
}

function text() {
  return host.textContent ?? "";
}

afterEach(() => {
  act(() => root.unmount());
  host.textContent = "";
  vi.clearAllMocks();
  vi.useRealTimers();
});

function mount(onOpenFile = vi.fn()) {
  backend.invoke.mockImplementation(async (cmd: string) => {
    if (cmd === "fs_tree") return TREE;
    if (cmd === "fs_search") return { results: [], truncated: false };
    throw new Error(`unmocked ${cmd}`);
  });
  act(() => {
    root = createRoot(host);
    root.render(
      <FilesPanel
        root="/repo"
        label="lazed"
        branch="main"
        paneCount={3}
        onClose={() => {}}
        onOpenFile={onOpenFile}
      />,
    );
  });
  return onOpenFile;
}

it("renders the tree: dirs, badges, ignored collapse, footer", async () => {
  mount();
  await flush();
  await flush();

  // collapsed by default — top-level entries only
  expect(text()).toContain("src");
  expect(text()).toContain("README.md");
  expect(text()).toContain("new.txt");
  expect(text()).toContain("dist");
  expect(text()).not.toContain("App.tsx"); // inside collapsed src
  expect(text()).toContain("main");
  expect(text()).toContain("3 panes");

  // ignored dir does not expand
  const dist = [...host.querySelectorAll(".file-row")].find((r) =>
    r.textContent?.includes("dist"),
  );
  act(() => (dist as HTMLElement).click());
  await flush();
  expect(text()).not.toContain("x.js");
});

it("expands a dir and opens a file via onOpenFile", async () => {
  const onOpenFile = mount();
  await flush();
  await flush();

  const src = [...host.querySelectorAll(".file-row")].find((r) =>
    r.textContent?.includes("src"),
  );
  act(() => (src as HTMLElement).click());
  await flush();
  expect(text()).toContain("App.tsx");

  const file = [...host.querySelectorAll(".file-row")].find((r) =>
    r.textContent?.includes("App.tsx"),
  );
  act(() => (file as HTMLElement).click());
  // the panel delegates — the file view (main area) does the reading
  expect(onOpenFile).toHaveBeenCalledWith("src/App.tsx", "M", "/repo");
  expect(backend.invoke).not.toHaveBeenCalledWith("fs_read", expect.anything());
});

it("filters by name when the query is set", async () => {
  mount();
  await flush();
  await flush();

  const input = host.querySelector(".files-input") as HTMLInputElement;
  act(() => {
    const setter = Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )?.set;
    setter?.call(input, "readme");
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await flush();
  expect(text()).toContain("README.md");
  expect(text()).not.toContain("new.txt");
});

function editQuery(value: string) {
  const input = host.querySelector(".files-input") as HTMLInputElement;
  act(() => {
    Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )?.set?.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

it("discards old search responses after a new query or workspace switch", async () => {
  vi.useFakeTimers();
  mount();
  await flush();
  const responses = new Map<string, (value: unknown) => void>();
  backend.invoke.mockImplementation(
    (cmd: string, args: { query: string; root: string }) => {
      if (cmd === "fs_tree")
        return Promise.resolve({ ...TREE, root: args.root });
      if (cmd === "fs_search")
        return new Promise((resolve) => responses.set(args.query, resolve));
      return Promise.reject("watch unavailable");
    },
  );
  act(() =>
    [...host.querySelectorAll("button")]
      .find((b) => b.textContent === "Contents")
      ?.click(),
  );
  editQuery("old");
  await act(async () => vi.advanceTimersByTime(200));
  editQuery("new");
  await act(async () => vi.advanceTimersByTime(200));
  await act(async () =>
    responses.get("new")?.({
      results: [{ path: "new.txt", line: 1, text: "new hit" }],
    }),
  );
  await act(async () =>
    responses.get("old")?.({
      results: [{ path: "old.txt", line: 1, text: "old hit" }],
    }),
  );
  expect(text()).toContain("new.txt");
  expect(text()).not.toContain("old.txt");
  editQuery("pending");
  await act(async () => vi.advanceTimersByTime(200));
  await act(async () =>
    root.render(
      <FilesPanel root="/another" onClose={() => {}} onOpenFile={() => {}} />,
    ),
  );
  await act(async () =>
    responses.get("pending")?.({
      results: [{ path: "stale.txt", line: 1, text: "stale" }],
    }),
  );
  expect(text()).not.toContain("stale.txt");
});

it("opens paths against the returned tree root rather than the workspace subdirectory", async () => {
  const onOpen = mount();
  await flush();
  backend.invoke.mockImplementation(async (cmd: string) => {
    if (cmd === "fs_tree") return TREE;
    throw new Error("watch unavailable");
  });
  await act(async () =>
    root.render(
      <FilesPanel root="/repo/subdir" onClose={() => {}} onOpenFile={onOpen} />,
    ),
  );
  act(() =>
    [...host.querySelectorAll(".file-row")]
      .find((b) => b.textContent?.includes("README.md"))
      ?.dispatchEvent(new MouseEvent("click", { bubbles: true })),
  );
  expect(onOpen).toHaveBeenCalledWith("README.md", undefined, "/repo");
});

it("debounces watch events, coalesces in-flight reloads and releases the watcher", async () => {
  vi.useFakeTimers();
  mount();
  await flush();
  backend.invoke.mockImplementation(async (cmd: string) => {
    if (cmd === "fs_tree") return TREE;
    if (cmd === "fs_watch") return { watch_id: "watch-2", root: "/repo" };
    return null;
  });
  await act(async () =>
    root.render(
      <FilesPanel root="/repo/new" onClose={() => {}} onOpenFile={() => {}} />,
    ),
  );
  await flush();
  const initial = backend.invoke.mock.calls.filter(
    ([cmd]) => cmd === "fs_tree",
  ).length;
  for (let i = 0; i < 20; i++)
    act(() => backend.event({ payload: { watch_id: "watch-2" } }));
  await act(async () => vi.advanceTimersByTime(250));
  expect(
    backend.invoke.mock.calls.filter(([cmd]) => cmd === "fs_tree").length,
  ).toBe(initial + 1);
  await act(async () => vi.advanceTimersByTime(12_000));
  expect(
    backend.invoke.mock.calls.filter(([cmd]) => cmd === "fs_tree").length,
  ).toBe(initial + 1);
  await act(async () =>
    root.render(
      <FilesPanel root="/different" onClose={() => {}} onOpenFile={() => {}} />,
    ),
  );
  expect(backend.invoke).toHaveBeenCalledWith("fs_unwatch", {
    watchId: "watch-2",
  });
});

it("keeps one tree request in flight and schedules only one followup for a refresh burst", async () => {
  mount();
  await flush();
  await flush();
  const before = backend.invoke.mock.calls.filter(
    ([cmd]) => cmd === "fs_tree",
  ).length;
  const pending: ((tree: FsTree) => void)[] = [];
  backend.invoke.mockImplementation((cmd: string) =>
    cmd === "fs_tree"
      ? new Promise((resolve) => pending.push(resolve))
      : Promise.resolve(null),
  );
  const refresh = host.querySelector('[title="refresh"]') as HTMLButtonElement;
  act(() => {
    for (let i = 0; i < 20; i++) refresh.click();
  });
  expect(
    backend.invoke.mock.calls.filter(([cmd]) => cmd === "fs_tree").length,
  ).toBe(before + 1);
  await act(async () => pending[0](TREE));
  expect(
    backend.invoke.mock.calls.filter(([cmd]) => cmd === "fs_tree").length,
  ).toBe(before + 2);
  await act(async () => pending[1](TREE));
  expect(
    backend.invoke.mock.calls.filter(([cmd]) => cmd === "fs_tree").length,
  ).toBe(before + 2);
});
