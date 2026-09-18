// @vitest-environment jsdom
import { act, createElement } from "react";
import { type Root, createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve({ ok: true, items: [] })),
  Channel: vi.fn(),
}));

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

import type { AutomationInput } from "../shared/automations";
import type { ProjectInfo } from "../shared/lazed";
import { AutomationEditor } from "./AutomationEditor";

const PROJECTS: ProjectInfo[] = [
  {
    project_id: "p1",
    label: "demo",
    repo_root: "/tmp/demo",
    repo_key: "demo",
    workspaces: [],
  },
];

let host: HTMLDivElement | null = null;
let root: Root | null = null;

function must<T>(v: T | null | undefined, what: string): T {
  if (v == null) throw new Error(`missing ${what}`);
  return v;
}

function mount(
  props: Partial<Parameters<typeof AutomationEditor>[0]> = {},
): HTMLDivElement {
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
  const all = {
    projects: PROJECTS,
    onSave: () => {},
    onDeleteSeen: () => {},
    onClose: () => {},
    ...props,
  };
  const r = root;
  act(() => r.render(createElement(AutomationEditor, all)));
  return host;
}

function q<T extends Element>(el: HTMLElement, sel: string): T {
  return must(el.querySelector<T>(sel), sel);
}

function chip(el: HTMLElement, tag: string): HTMLButtonElement {
  const chips = el.querySelectorAll<HTMLButtonElement>(
    ".autoedit-presets .fanout-kind",
  );
  return must(
    [...chips].find((c) => c.textContent === tag),
    `chip ${tag}`,
  );
}

function saveBtn(el: HTMLElement): HTMLButtonElement {
  return q<HTMLButtonElement>(el, ".autoedit-save");
}

afterEach(() => {
  act(() => root?.unmount());
  host?.remove();
  host = null;
  root = null;
});

describe("AutomationEditor", () => {
  it("mounts with title, sections, and action cards", () => {
    const el = mount();
    expect(el.querySelector(".autoedit-title")?.textContent).toBe(
      "New automation",
    );
    expect(el.querySelectorAll(".autoedit-action").length).toBe(5);
    expect(el.querySelectorAll(".autoedit-presets .fanout-kind").length).toBe(
      5,
    );
    expect(saveBtn(el).disabled).toBe(true);
  });

  it("fills the command and selects the chip when a preset is applied", () => {
    const el = mount();
    const gh = chip(el, "github");
    act(() => gh.click());
    const cmd = q<HTMLTextAreaElement>(el, ".autoedit-cmd");
    expect(cmd.value).toContain("gh search prs");
    expect(gh.className).toContain("sel");
    expect(saveBtn(el).disabled).toBe(false);
  });

  it("names the automation and shows the hint for the jira mention preset", () => {
    const el = mount();
    act(() => chip(el, "jira @me").click());
    const cmd = q<HTMLTextAreaElement>(el, ".autoedit-cmd");
    expect(cmd.value).toContain("focusedCommentId");
    expect(q<HTMLInputElement>(el, ".autoedit-field").value).toBe(
      "Jira mention @me",
    );
    expect(el.querySelector(".autoedit-hint")?.textContent).toContain(
      "@-mention",
    );
  });

  it("submits an automation with hours converted to seconds", () => {
    const saved: AutomationInput[] = [];
    const el = mount({
      onSave: (i) => {
        saved.push(i);
      },
    });
    act(() => chip(el, "github").click());
    const unit = q<HTMLSelectElement>(el, ".autoedit-unit");
    act(() => {
      unit.value = "h";
      unit.dispatchEvent(new Event("change", { bubbles: true }));
    });
    act(() => saveBtn(el).click());
    expect(saved.length).toBe(1);
    expect(saved[0].interval_secs).toBe(18000);
    expect(saved[0].command).toContain("gh search prs");
    expect(saved[0].action.kind).toBe("notify");
  });

  it("requires a command template for the command action", () => {
    const el = mount();
    act(() => chip(el, "github").click());
    const card = must(
      [...el.querySelectorAll<HTMLButtonElement>(".autoedit-action")].find(
        (b) => b.textContent === "Command",
      ),
      "command card",
    );
    act(() => card.click());
    expect(saveBtn(el).disabled).toBe(true);
  });
});
