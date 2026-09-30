// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, it, vi } from "vitest";
const backend = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: backend.invoke }));
import { DiffView } from "./DiffView";
(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;
const host = document.createElement("div");
document.body.append(host);
let root: ReturnType<typeof createRoot>;
afterEach(() => {
  act(() => root.unmount());
  host.textContent = "";
  vi.clearAllMocks();
});
async function mount(prompt: () => Promise<unknown>) {
  backend.invoke.mockImplementation((cmd: string) =>
    cmd === "worktree_diff"
      ? Promise.resolve({
          branch: "fixture",
          diff: "diff --git a/f b/f\n@@ -0,0 +1,1 @@\n+hello\n",
          stat: "",
          untracked: [],
        })
      : prompt(),
  );
  root = createRoot(host);
  await act(async () =>
    root.render(
      <DiffView
        checkout="/repo"
        workspaceId="w1"
        agentTermId="w1:p1"
        onClose={() => {}}
        onJump={() => {}}
      />,
    ),
  );
}
function add(text: string) {
  act(() =>
    (host.querySelector(".diff-line.add") as HTMLButtonElement).click(),
  );
  const input = host.querySelector(".diff-draft input") as HTMLInputElement;
  act(() => {
    Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )?.set?.call(input, text);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  act(() =>
    (host.querySelector(".diff-draft button") as HTMLButtonElement).click(),
  );
}
it("retains review comments when the agent refuses the submission", async () => {
  await mount(() => Promise.reject("agent_blocked"));
  add("keep my review");
  await act(async () =>
    (host.querySelector(".fanout-go") as HTMLButtonElement).click(),
  );
  expect(host.querySelector(".diff-comments")?.textContent).toContain(
    "keep my review",
  );
  expect(host.textContent).toContain("agent_blocked");
});
it("submits once and preserves comments added while awaiting the receipt", async () => {
  let resolve!: (value: unknown) => void;
  await mount(
    () =>
      new Promise((done) => {
        resolve = done;
      }),
  );
  add("submitted");
  const send = host.querySelector(".fanout-go") as HTMLButtonElement;
  act(() => {
    send.click();
    send.click();
  });
  expect(
    backend.invoke.mock.calls.filter(([cmd]) => cmd === "agent_prompt"),
  ).toHaveLength(1);
  add("new unsent comment");
  await act(async () => resolve({ accepted: true }));
  expect(host.querySelector(".diff-comments")?.textContent).toContain(
    "new unsent comment",
  );
  expect(host.querySelector(".diff-comments")?.textContent).not.toContain(
    "submitted",
  );
});
