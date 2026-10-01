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
it("groups open items into collapsible provider sections", async () => {
  root = createRoot(host);
  await act(async () =>
    root.render(
      <InboxView
        items={[
          {
            id: "i1",
            title: "assigned issue",
            source: "Jira — issues assigned to me (REST)",
            at: 3,
            status: "open",
          },
          {
            id: "i2",
            title: "mention",
            source: "Jira mention @me",
            provider: "jira",
            at: 2,
            status: "open",
          },
          {
            id: "i3",
            title: "channel msg",
            source: "Slack — channel messages (REST)",
            at: 1,
            status: "open",
          },
          {
            id: "i4",
            title: "quick capture",
            source: "manual",
            at: 0,
            status: "open",
          },
        ]}
        projects={[]}
        onChanged={() => {}}
        onFlash={() => {}}
        onError={() => {}}
      />,
    ),
  );
  const heads = [...host.querySelectorAll(".ibx-group-head")];
  expect(heads.map((h) => h.textContent)).toEqual([
    "▾Jira2",
    "▾Slack1",
    "▾Captured1",
  ]);
  expect(host.querySelectorAll(".ibx-row").length).toBe(4);
  // collapsing a section hides its rows
  await act(async () => (heads[0] as HTMLButtonElement).click());
  expect(host.querySelectorAll(".ibx-row").length).toBe(2);
});

it("clusters jira rows under a shared issue head", async () => {
  backend.invoke.mockResolvedValue({});
  root = createRoot(host);
  await act(async () =>
    root.render(
      <InboxView
        items={[
          {
            id: "i1",
            title: "CS-1 assigned",
            source: "Jira — issues assigned to me (REST)",
            key: "CS-1",
            at: 3,
            status: "open",
          },
          {
            id: "i2",
            title: "CS-1#9 summary · someone: ping",
            source: "Jira mention @me",
            provider: "jira",
            key: "CS-1#9",
            at: 2,
            status: "open",
          },
          {
            id: "i3",
            title: "CS-2 assigned",
            source: "Jira — issues assigned to me (REST)",
            key: "CS-2",
            at: 1,
            status: "open",
          },
        ]}
        projects={[]}
        onChanged={() => {}}
        onFlash={() => {}}
        onError={() => {}}
      />,
    ),
  );
  const keys = [...host.querySelectorAll(".ibx-issue-key")].map(
    (el) => el.textContent,
  );
  expect(keys).toEqual(["CS-1", "CS-2"]);
  expect(host.querySelectorAll(".ibx-row").length).toBe(3);
  // the issue head's check marks every row in the cluster done
  await act(async () =>
    (
      host.querySelector('[title="mark all done"]') as HTMLButtonElement
    ).click(),
  );
  const updates = backend.invoke.mock.calls.filter(
    ([cmd]) => cmd === "inbox_update",
  );
  expect(updates.length).toBe(2);
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
