import { expect, it, vi } from "vitest";
import { createSnapshotLoader } from "./snapshot-loader";

it("starts a fresh read when a refresh arrives between publication and promise cleanup", async () => {
  const read = vi
    .fn()
    .mockResolvedValueOnce("first")
    .mockResolvedValueOnce("second");
  let next: Promise<string> | undefined;
  const loader = createSnapshotLoader<string>(read, (value) => {
    if (value === "first")
      queueMicrotask(() => {
        next = loader.load();
      });
  });
  expect(await loader.load()).toBe("first");
  expect(await next).toBe("second");
  expect(read).toHaveBeenCalledTimes(2);
});

it("serializes requests and publishes only a response newer than the last event", async () => {
  const pending: ((value: string) => void)[] = [];
  const read = vi.fn(
    () => new Promise<string>((resolve) => pending.push(resolve)),
  );
  const publish = vi.fn();
  const loader = createSnapshotLoader(read, publish);
  const first = loader.load();
  loader.invalidate();
  const second = loader.load();
  expect(read).toHaveBeenCalledTimes(1);
  pending[0]("stale");
  await Promise.resolve();
  expect(publish).not.toHaveBeenCalled();
  expect(read).toHaveBeenCalledTimes(2);
  pending[1]("current");
  expect(await first).toBe("current");
  expect(await second).toBe("current");
  expect(publish).toHaveBeenCalledExactlyOnceWith("current");
});

it("releases a failed request so a later refresh can recover", async () => {
  const read = vi
    .fn()
    .mockRejectedValueOnce(new Error("offline"))
    .mockResolvedValueOnce("recovered");
  const publish = vi.fn();
  const loader = createSnapshotLoader(read, publish);
  await expect(loader.load()).rejects.toThrow("offline");
  await expect(loader.load()).resolves.toBe("recovered");
  expect(publish).toHaveBeenCalledExactlyOnceWith("recovered");
});
