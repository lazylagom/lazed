// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, it, vi } from "vitest";

const dialog = vi.hoisted(() => ({ open: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: dialog.open }));

import { ImportProject } from "./ImportProject";

type OnImport = (cwd?: string, label?: string, initSkills?: boolean) => void;

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;
const host = document.createElement("div");
document.body.append(host);
let root: ReturnType<typeof createRoot>;

afterEach(() => {
  act(() => root.unmount());
  host.textContent = "";
  dialog.open.mockReset();
});

async function browse(onImport: OnImport, uncheck = false) {
  dialog.open.mockResolvedValue("/work/shop");
  root = createRoot(host);
  act(() =>
    root.render(<ImportProject onImport={onImport} onClose={() => {}} />),
  );
  const box = document.querySelector<HTMLInputElement>(".addproj-opt input");
  expect(box?.checked).toBe(true);
  if (uncheck) act(() => box?.click());
  await act(async () => {
    document.querySelector<HTMLButtonElement>(".addproj-card")?.click();
  });
}

it("installs the crew skill by default", async () => {
  const onImport = vi.fn<OnImport>();
  await browse(onImport);
  expect(onImport).toHaveBeenCalledWith("/work/shop", "shop", true);
});

it("skips the crew skill when unchecked", async () => {
  const onImport = vi.fn<OnImport>();
  await browse(onImport, true);
  expect(onImport).toHaveBeenCalledWith("/work/shop", "shop", false);
});
