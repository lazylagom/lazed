// @vitest-environment jsdom
import { type ReactNode, act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AgentPicker } from "../features/AgentPicker";
import { ImportProject } from "../features/ImportProject";
import { PromptBar } from "../features/PromptBar";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
import { open } from "@tauri-apps/plugin-dialog";

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;
let host: HTMLDivElement;
let root: ReturnType<typeof createRoot>;
function render(view: ReactNode) {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  act(() => root.render(view));
}
function element(selector: string): HTMLElement {
  const found = host.querySelector<HTMLElement>(selector);
  if (!found) throw new Error(`Missing element: ${selector}`);
  return found;
}
function key(target: Element | Window, value: string) {
  act(() => {
    target.dispatchEvent(
      new KeyboardEvent("keydown", { key: value, bubbles: true }),
    );
  });
}
afterEach(() => {
  act(() => root.unmount());
  host.remove();
  vi.clearAllMocks();
});

const views = {
  agent: (close: () => void) => (
    <AgentPicker onClose={close} onPick={() => {}} />
  ),
  prompt: (close: () => void) => (
    <PromptBar
      onClose={close}
      onSubmit={() => {}}
      focusedTerm="t1"
      agentCount={1}
      termCount={2}
    />
  ),
  import: (close: () => void) => (
    <ImportProject onClose={close} onImport={() => {}} />
  ),
};

describe.each(Object.entries(views))("%s modal", (_name, view) => {
  it("closes on the backdrop but keeps inside interactions open", () => {
    const close = vi.fn();
    render(view(close));
    act(() => {
      host
        .querySelector(".modal")
        ?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    });
    expect(close).not.toHaveBeenCalled();
    act(() => {
      host
        .querySelector(".modal-overlay")
        ?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    });
    expect(close).toHaveBeenCalledTimes(1);
  });
  it("closes once when Escape bubbles from its contents", () => {
    const close = vi.fn();
    render(view(close));
    key(element("input, button"), "Escape");
    expect(close).toHaveBeenCalledTimes(1);
  });
});

it("focuses agent search and preserves arrow/Enter selection", () => {
  const pick = vi.fn();
  render(<AgentPicker onClose={() => {}} onPick={pick} />);
  const input = element("input");
  expect(document.activeElement).toBe(input);
  key(input, "ArrowDown");
  const selected = element(".modal-item.sel").textContent;
  key(input, "Enter");
  expect(pick).toHaveBeenCalledWith(selected);
});

it("focuses the prompt and submits trimmed text to the focused terminal", () => {
  const submit = vi.fn();
  const close = vi.fn();
  render(
    <PromptBar
      onClose={close}
      onSubmit={submit}
      focusedTerm="t1"
      agentCount={1}
      termCount={2}
    />,
  );
  const input = element("input");
  expect(document.activeElement).toBe(input);
  act(() => {
    Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )?.set?.call(input, "  review changes  ");
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  key(input, "Enter");
  expect(submit).toHaveBeenCalledWith("review changes", {
    kind: "focused",
    termId: "t1",
  });
  expect(close).toHaveBeenCalledTimes(1);
});

it("retains import styling, folder selection and the window Escape listener lifecycle", async () => {
  const close = vi.fn();
  const imported = vi.fn();
  vi.mocked(open).mockResolvedValue("/tmp/project/");
  render(<ImportProject onClose={close} onImport={imported} />);
  expect(host.querySelector(".modal.addproj")).not.toBeNull();
  await act(async () => {
    element(".addproj-card").click();
  });
  expect(imported).toHaveBeenCalledWith("/tmp/project/", "project");
  key(window, "Escape");
  expect(close).toHaveBeenCalledTimes(1);
  act(() => root.render(null));
  key(window, "Escape");
  expect(close).toHaveBeenCalledTimes(1);
});
