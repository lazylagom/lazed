// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, it, vi } from "vitest";
const backend = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: backend.invoke }));
import { InboxView } from "./InboxView";
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
it.each([null, { accepted: false }, { accepted: true }])(
  "archives delegated capture only with an accepted receipt: %s",
  async (receipt) => {
    const onError = vi.fn();
    backend.invoke.mockResolvedValue({
      task_id: "task",
      phase: receipt?.accepted ? "running" : "submission_uncertain",
      receipt,
    });
    root = createRoot(host);
    await act(async () =>
      root.render(
        <InboxView
          items={[
            {
              id: "i1",
              title: "capture",
              source: "manual",
              at: 1,
              status: "open",
            },
          ]}
          projects={[
            {
              project_id: "p1",
              repo_root: "/repo",
              repo_key: "/repo/.git",
              workspaces: [],
            },
          ]}
          focusedProjectId="p1"
          onChanged={() => {}}
          onFlash={() => {}}
          onError={onError}
        />,
      ),
    );
    act(() =>
      (
        host.querySelector(
          '[title="delegate to agent / pane"]',
        ) as HTMLButtonElement
      ).click(),
    );
    await act(async () =>
      [...host.querySelectorAll("button")]
        .find((b) => b.textContent?.includes("spawn task"))
        ?.click(),
    );
    const archived = backend.invoke.mock.calls.some(
      ([cmd]) => cmd === "inbox_update",
    );
    expect(archived).toBe(receipt?.accepted === true);
    if (!receipt?.accepted)
      expect(onError).toHaveBeenCalledWith(
        expect.stringContaining("submission_uncertain"),
      );
  },
);
