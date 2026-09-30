import { expect, it, vi } from "vitest";
import type { Snapshot } from "./lazed";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  Channel: class {},
}));

import { normalize } from "./lazed";

const pane = (pane_id: string, tab_id: string, workspace_id = "w1") => ({
  pane_id,
  workspace_id,
  tab_id,
});

it("orders tab panes by layout order, then unlisted panes in pane order", () => {
  const snap: Snapshot = {
    projects: [],
    panes: [
      pane("p1", "t1"),
      pane("p2", "t1"),
      pane("p3", "t1"),
      pane("q1", "t2"),
    ],
    tabs: [
      { tab_id: "t2", workspace_id: "w1", panes: [] },
      { tab_id: "t1", workspace_id: "w1", panes: [] },
    ],
    workspaces: [],
    layouts: [
      {
        tab_id: "t1",
        workspace_id: "w1",
        panes: [{ pane_id: "p3" }, { pane_id: "p1" }],
      },
    ],
  };
  const out = normalize(snap);
  const t1 = out.tabs?.find((t) => t.tab_id === "t1");
  // layout order first; p2 had no layout entry and trails in pane order
  expect(t1?.panes).toEqual(["p3", "p1", "p2"]);
});

it("drops layout entries for panes that left the tab and ignores unknown ids", () => {
  const snap: Snapshot = {
    projects: [],
    panes: [pane("p1", "t1")],
    tabs: [{ tab_id: "t1", workspace_id: "w1", panes: [] }],
    workspaces: [],
    layouts: [
      {
        tab_id: "t1",
        workspace_id: "w1",
        panes: [{ pane_id: "ghost" }, { pane_id: "p1" }],
      },
    ],
  };
  expect(normalize(snap).tabs?.[0]?.panes).toEqual(["p1"]);
});

it("fills workspace tabs in sorted tab order and merges agent names", () => {
  const snap: Snapshot = {
    projects: [],
    panes: [pane("p1", "t2"), pane("p2", "t1")],
    tabs: [
      { tab_id: "t1", workspace_id: "w1", number: 1, panes: [] },
      { tab_id: "t2", workspace_id: "w1", number: 2, panes: [] },
      { tab_id: "t9", workspace_id: "w2", number: 0, panes: [] },
    ],
    workspaces: [
      { workspace_id: "w1", tabs: [] },
      { workspace_id: "w2", tabs: [] },
    ],
    agents: [{ pane_id: "p2", name: "claude-p2" }],
  };
  const out = normalize(snap);
  expect(out.workspaces?.[0]?.tabs).toEqual(["t1", "t2"]);
  expect(out.workspaces?.[1]?.tabs).toEqual(["t9"]);
  expect(out.panes[1]?.agent_name).toBe("claude-p2");
});
