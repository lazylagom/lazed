// @vitest-environment jsdom
import { act, createElement } from "react";
import { type Root, createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn((_cmd: string, _args?: unknown) =>
  Promise.resolve({ bins: {}, env: {} }),
);
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: unknown) => invoke(cmd, args),
  Channel: vi.fn(),
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ startDragging: () => Promise.resolve() }),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({
  ask: () => Promise.resolve(false),
}));

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

import { AUTOMATION_PRESETS, type Automation } from "../shared/automations";
import { Automations } from "./Automations";

const JIRA_MENTION = AUTOMATION_PRESETS.find((p) => p.id === "jira-mention");
if (!JIRA_MENTION) throw new Error("jira-mention preset missing");

function auto(over: Partial<Automation>): Automation {
  return {
    id: "a1",
    name: "x",
    enabled: true,
    interval_secs: 300,
    command: "true",
    action: { kind: "inbox" },
    seen_count: 0,
    seeded: true,
    last_items: [],
    fire_count: 0,
    ...over,
  };
}

let host: HTMLDivElement | null = null;
let root: Root | null = null;

function must<T>(v: T | null | undefined, what: string): T {
  if (v == null) throw new Error(`missing ${what}`);
  return v;
}

function mount(autos: Automation[]): HTMLDivElement {
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
  const r = root;
  act(() =>
    r.render(
      createElement(Automations, {
        autos,
        onNew: () => {},
        onEdit: () => {},
        onChanged: () => {},
        editorOpen: false,
        onClose: () => {},
      }),
    ),
  );
  return host;
}

function catalogRow(el: HTMLElement, preset: string): HTMLElement {
  return must(
    el.querySelector<HTMLElement>(`.auto-catalog[data-preset="${preset}"]`),
    `catalog row ${preset}`,
  );
}

function toggleOf(row: HTMLElement): HTMLButtonElement {
  return must(row.querySelector<HTMLButtonElement>(".set-toggle"), "toggle");
}

const flush = () => act(async () => {});

beforeEach(() => invoke.mockClear());

afterEach(() => {
  act(() => root?.unmount());
  host?.remove();
  host = null;
  root = null;
});

describe("Automations catalog", () => {
  it("lists every ready-made preset with an off switch when nothing exists", () => {
    const el = mount([]);
    const rows = el.querySelectorAll(".auto-catalog");
    expect(rows.length).toBe(
      AUTOMATION_PRESETS.filter((p) => p.command).length,
    );
    const row = catalogRow(el, "jira-mention");
    expect(row.textContent).toContain("Jira mention @me");
    expect(toggleOf(row).getAttribute("aria-checked")).toBe("false");
    expect(el.querySelector(".set-section-title + .set-card")).not.toBeNull();
    // no Custom section without hand-made automations
    expect(
      [...el.querySelectorAll(".set-section-title")].map((n) => n.textContent),
    ).toEqual(["Ready-made"]);
  });

  it("switching a preset on creates it from the preset defaults", async () => {
    const el = mount([]);
    act(() => toggleOf(catalogRow(el, "jira-mention")).click());
    await flush();
    const save = invoke.mock.calls.find(([cmd]) => cmd === "automation_save");
    const input = (save?.[1] as { input: Record<string, unknown> }).input;
    expect(input).toMatchObject({
      name: "Jira mention @me",
      enabled: true,
      interval_secs: 300,
      command: JIRA_MENTION.command,
      action: { kind: "inbox" },
      preset: "jira-mention",
    });
  });

  it("maps a stored automation onto its preset and pauses it on toggle", async () => {
    const stored = auto({
      id: "a9",
      name: "my jira pings",
      preset: "jira-mention",
      command: "edited",
    });
    const el = mount([stored]);
    const row = catalogRow(el, "jira-mention");
    expect(row.textContent).toContain("my jira pings");
    expect(toggleOf(row).getAttribute("aria-checked")).toBe("true");
    // mapped automations don't show up again under Custom
    expect(el.querySelectorAll(".set-section-title").length).toBe(1);

    act(() => toggleOf(row).click());
    await flush();
    expect(invoke).toHaveBeenCalledWith("automation_set_enabled", {
      id: "a9",
      enabled: false,
    });
  });

  it("matches a legacy automation by its unedited preset command", () => {
    const el = mount([auto({ id: "old", command: JIRA_MENTION.command })]);
    expect(
      toggleOf(catalogRow(el, "jira-mention")).getAttribute("aria-checked"),
    ).toBe("true");
  });

  it("lists hand-made automations under Custom with their own switch", async () => {
    const el = mount([
      auto({ id: "c1", name: "cron thing", command: "ls", enabled: false }),
    ]);
    const titles = [...el.querySelectorAll(".set-section-title")].map(
      (n) => n.textContent,
    );
    expect(titles).toEqual(["Ready-made", "Custom"]);
    const row = must(
      [...el.querySelectorAll<HTMLElement>(".auto-row:not(.auto-catalog)")][0],
      "custom row",
    );
    expect(row.textContent).toContain("cron thing");
    act(() => toggleOf(row).click());
    await flush();
    expect(invoke).toHaveBeenCalledWith("automation_set_enabled", {
      id: "c1",
      enabled: true,
    });
  });
});
