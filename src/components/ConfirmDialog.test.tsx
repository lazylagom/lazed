// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, expect, it } from "vitest";
import { ConfirmHost, confirmDialog } from "./ConfirmDialog";

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;
let host: HTMLDivElement;
let root: ReturnType<typeof createRoot>;
beforeEach(() => {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  act(() => root.render(<ConfirmHost />));
});
afterEach(() => {
  act(() => root.unmount());
  host.remove();
});

function open() {
  let p!: Promise<boolean>;
  act(() => {
    p = confirmDialog({
      title: "Remove Workspace",
      message: "Remove it?",
      detail: "/tmp/wt",
      okLabel: "Remove",
      danger: true,
    });
  });
  return p;
}

it("renders themed, focuses confirm and resolves true on click", async () => {
  const p = open();
  const ok = host.querySelector<HTMLButtonElement>(".confirm-btn.primary");
  expect(host.querySelector(".modal.confirm")).not.toBeNull();
  expect(host.querySelector(".confirm-detail")?.textContent).toBe("/tmp/wt");
  expect(document.activeElement).toBe(ok);
  act(() => ok?.click());
  await expect(p).resolves.toBe(true);
  expect(host.querySelector(".modal")).toBeNull();
});

it("resolves false on Escape and on the backdrop", async () => {
  const a = open();
  act(() => {
    host
      .querySelector(".confirm-btn")
      ?.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      );
  });
  await expect(a).resolves.toBe(false);
  const b = open();
  act(() => {
    host
      .querySelector(".modal-overlay")
      ?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
  });
  await expect(b).resolves.toBe(false);
});
