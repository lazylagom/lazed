// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { lazed } from "../shared/lazed";
import { DaemonUpdate } from "./DaemonUpdate";

const ask = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/plugin-dialog", () => ({ ask }));
vi.mock("../shared/lazed", () => ({
  lazed: { status: vi.fn(), restartServer: vi.fn() },
}));
(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;
let host: HTMLDivElement;
let root: ReturnType<typeof createRoot>;
const stale = {
  running: true,
  binary_updated: true,
  pid: 10,
  terms: 0,
  capabilities: ["server.stop_if_empty.v1"],
};

beforeEach(() => {
  vi.useFakeTimers();
  vi.resetAllMocks();
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  vi.mocked(lazed.restartServer).mockResolvedValue({
    ...stale,
    binary_updated: false,
    pid: 11,
  });
});
afterEach(() => {
  act(() => root.unmount());
  host.remove();
  vi.useRealTimers();
});
async function render() {
  await act(async () => root.render(<DaemonUpdate />));
}
async function clickRestart() {
  await act(async () => host.querySelector("button")?.click());
}

it("automatically restarts only an empty daemon using the guarded API", async () => {
  vi.mocked(lazed.status).mockResolvedValue(stale);
  await render();
  expect(lazed.restartServer).toHaveBeenCalledExactlyOnceWith(true);
  expect(ask).not.toHaveBeenCalled();
  expect(host.textContent).toBe("");
});

it("polls for builds and waits for confirmation when terminals exist", async () => {
  vi.mocked(lazed.status).mockResolvedValue({
    ...stale,
    binary_updated: false,
  });
  await render();
  expect(host.textContent).toBe("");
  vi.mocked(lazed.status).mockResolvedValue({ ...stale, terms: 2 });
  await act(async () => vi.advanceTimersByTimeAsync(5_000));
  expect(host.textContent).toContain("2 terminal(s)");
  expect(lazed.restartServer).not.toHaveBeenCalled();
  ask.mockResolvedValueOnce(false);
  await clickRestart();
  expect(lazed.restartServer).not.toHaveBeenCalled();
  ask.mockResolvedValueOnce(true);
  await clickRestart();
  expect(lazed.restartServer).toHaveBeenCalledExactlyOnceWith();
});

it("requires manual confirmation for an old daemon without the safety gate", async () => {
  vi.mocked(lazed.status).mockResolvedValue({ ...stale, capabilities: [] });
  await render();
  expect(host.textContent).toContain("Daemon update available");
  expect(lazed.restartServer).not.toHaveBeenCalled();
});

it("keeps a raced or failed automatic restart visible without retry loops", async () => {
  vi.mocked(lazed.status).mockResolvedValue(stale);
  vi.mocked(lazed.restartServer).mockRejectedValue("daemon_has_terminals");
  await render();
  expect(host.textContent).toContain("daemon_has_terminals");
  await act(async () => vi.advanceTimersByTimeAsync(15_000));
  expect(lazed.restartServer).toHaveBeenCalledTimes(1);
});

it("automatically applies the build after the last terminal closes", async () => {
  vi.mocked(lazed.status).mockResolvedValue({ ...stale, terms: 1 });
  await render();
  vi.mocked(lazed.status).mockResolvedValue(stale);
  await act(async () => vi.advanceTimersByTimeAsync(5_000));
  expect(lazed.restartServer).toHaveBeenCalledExactlyOnceWith(true);
});
