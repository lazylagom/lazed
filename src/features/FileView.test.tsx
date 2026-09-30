// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, it, vi } from "vitest";

const backend = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: backend.invoke }));

import { FileView } from "./FileView";

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;
const host = document.createElement("div");
document.body.append(host);
let root: ReturnType<typeof createRoot>;

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
});

function mount(path: string, git?: "M" | "D" | "U" | "!" | null) {
  backend.invoke.mockImplementation(async (cmd: string) => {
    if (cmd === "fs_read")
      return { content: "# Title\n\nbody text\n", binary: false };
    if (cmd === "fs_diff")
      return {
        diff: "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-old\n+new\n",
      };
    throw new Error(`unmocked ${cmd}`);
  });
  act(() => {
    root = createRoot(host);
    root.render(
      <FileView root="/repo" path={path} git={git} onClose={() => {}} />,
    );
  });
}

it("shows text content with the abs path in the head", async () => {
  mount("src/f.txt", null);
  await flush();
  await flush();
  expect(text()).toContain("/repo/src/f.txt");
  expect(text()).toContain("body text");
});

it("leads with the diff for modified files", async () => {
  mount("src/f.txt", "M");
  await flush();
  await flush();
  await flush();
  expect(backend.invoke).toHaveBeenCalledWith(
    "fs_diff",
    expect.objectContaining({ path: "src/f.txt" }),
  );
  expect(host.querySelector(".diff-line.add")).toBeTruthy();
  expect(text()).toContain("+new");
});

it("renders markdown preview for .md files", async () => {
  mount("PLAN.md", null);
  await flush();
  await flush();
  await vi.waitFor(async () => {
    await flush();
    expect(host.querySelector(".fv-md")).toBeTruthy();
  });
  const md = host.querySelector(".fv-md");
  expect(md?.querySelector("h1")?.textContent).toBe("Title");
  // raw markdown syntax must not leak into the rendered preview
  expect(md?.textContent).not.toContain("# Title");
});
