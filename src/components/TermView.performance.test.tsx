// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, it, vi } from "vitest";
import { TERMINAL_THEME } from "../terminal/config";
import { TermView } from "./TermView";

const terminal = vi.hoisted(() => ({
  theme: vi.fn(),
  create: vi.fn(),
  dispose: vi.fn(),
  focus: vi.fn(),
}));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => {}),
  Channel: class {},
}));
vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    options = {
      set theme(value: unknown) {
        terminal.theme(value);
      },
    };
    unicode = { activeVersion: "" };
    cols = 80;
    rows = 24;
    constructor(options: unknown) {
      terminal.create(options);
    }
    loadAddon() {}
    open() {}
    onData() {
      return { dispose() {} };
    }
    dispose() {
      terminal.dispose();
    }
  },
}));
vi.mock("@xterm/addon-fit", () => ({
  FitAddon: class {
    fit() {}
  },
}));
vi.mock("@xterm/addon-unicode11", () => ({ Unicode11Addon: class {} }));
vi.mock("@xterm/addon-web-links", () => ({ WebLinksAddon: class {} }));
vi.mock("../terminal/ime-overlay", () => ({
  IMEOverlay: class {
    focus() {
      terminal.focus();
    }
    dispose() {}
  },
}));
(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;
let root: ReturnType<typeof createRoot>;
let host: HTMLDivElement;
afterEach(() => {
  act(() => root.unmount());
  host.remove();
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});
it("does not repaint the theme or recreate xterm for metadata and focus updates", () => {
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      disconnect() {}
    },
  );
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  const onFocus = vi.fn();
  const onClose = vi.fn();
  for (let i = 0; i < 20; i++) {
    act(() =>
      root.render(
        <TermView
          term={{ term_id: "t1", cwd: "/tmp", label: `pane ${i}` }}
          focused={i % 2 === 0}
          onFocus={onFocus}
          onClose={onClose}
        />,
      ),
    );
  }
  expect(terminal.create).toHaveBeenCalledTimes(1);
  expect(terminal.create).toHaveBeenCalledWith(
    expect.objectContaining({ theme: TERMINAL_THEME }),
  );
  expect(terminal.theme).not.toHaveBeenCalled();
  expect(terminal.focus).toHaveBeenCalledTimes(10);
  expect(host.textContent).toContain("pane 19");
  act(() =>
    root.render(
      <TermView
        term={{ term_id: "t2", cwd: "/tmp" }}
        focused={false}
        onFocus={onFocus}
        onClose={onClose}
      />,
    ),
  );
  expect(terminal.dispose).toHaveBeenCalledTimes(1);
  expect(terminal.create).toHaveBeenCalledTimes(2);
});
