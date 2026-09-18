// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, it, vi } from "vitest";
import { lazed } from "../shared/lazed";
import { DiffView } from "./DiffView";
import { parseDiff } from "./diff-parser";

vi.mock("../shared/lazed", () => ({ lazed: { worktreeDiff: vi.fn() } }));
vi.mock("./diff-parser", async (original) => {
  const actual = await original<typeof import("./diff-parser")>();
  return { ...actual, parseDiff: vi.fn(actual.parseDiff) };
});
(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;
const host = document.createElement("div");
document.body.append(host);
let root: ReturnType<typeof createRoot>;
afterEach(() => {
  act(() => root.unmount());
  vi.clearAllMocks();
});
function button(selector: string) {
  const found = host.querySelector<HTMLButtonElement>(selector);
  if (!found) throw new Error(`Missing ${selector}`);
  return found;
}
it("reuses parsed diff during comment edits and identical refreshes, then updates on changed diff", async () => {
  const diff = "diff --git a/f b/f\n@@ -0,0 +1,1 @@\n+hello\n";
  const response = { branch: "test", diff, stat: "", untracked: [] };
  vi.mocked(lazed.worktreeDiff).mockResolvedValue(response);
  root = createRoot(host);
  const identity = { termId: "t1", workspaceId: "w1" };
  await act(async () =>
    root.render(
      <DiffView
        checkout="/tmp"
        {...identity}
        onClose={() => {}}
        onJump={() => {}}
      />,
    ),
  );
  expect(parseDiff).toHaveBeenCalledTimes(1);
  act(() => button(".diff-line.add").click());
  const input = host.querySelector("input");
  if (!input) throw new Error("Missing comment input");
  for (let i = 1; i <= 20; i++) {
    act(() => {
      Object.getOwnPropertyDescriptor(
        HTMLInputElement.prototype,
        "value",
      )?.set?.call(input, "x".repeat(i));
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
  }
  expect(input.value).toHaveLength(20);
  expect(parseDiff).toHaveBeenCalledTimes(1);
  await act(async () => button('[title="refresh diff"]').click());
  expect(parseDiff).toHaveBeenCalledTimes(1);
  vi.mocked(lazed.worktreeDiff).mockResolvedValue({
    ...response,
    diff: diff.replace("hello", "updated"),
  });
  await act(async () => button('[title="refresh diff"]').click());
  expect(parseDiff).toHaveBeenCalledTimes(2);
  expect(host.textContent).toContain("+updated");
});
