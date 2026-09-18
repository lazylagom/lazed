// Launch-scoped integration; never installed into the user's global Pi profile.
import net from "node:net";
import os from "node:os";
import path from "node:path";

interface Context {
  mode: string;
  isIdle(): boolean;
}
interface Extension {
  on(
    event: string,
    handler: (event: unknown, context: Context) => Promise<void>,
  ): void;
}

export default function lazedLifecycle(pi: Extension) {
  const launch = process.env.LAZED_LAUNCH_ID;
  const term = process.env.LAZED_TERM;
  if (!launch || !term) return;
  const socket = path.join(
    process.env.LAZED_STATE_DIR ??
      path.join(os.homedir(), ".local/state/lazed"),
    "lazed.sock",
  );
  let seq = Date.now() * 1000;
  let pending = Promise.resolve();
  const report = (state: string) => {
    const params = { term_id: term, launch_id: launch, seq: ++seq, state };
    pending = pending.then(
      () =>
        new Promise<void>((resolve) => {
          const conn = net.createConnection(socket);
          const finish = () => {
            conn.destroy();
            resolve();
          };
          conn.setTimeout(2000, finish);
          conn.on("error", finish);
          conn.on("data", finish);
          conn.on("connect", () =>
            conn.write(
              `${JSON.stringify({ id: 1, method: "agent.report", params })}\n`,
            ),
          );
        }),
    );
    return pending;
  };
  pi.on("session_start", async (_event, ctx) => {
    if (ctx.mode === "tui" && ctx.isIdle()) await report("idle");
  });
  pi.on("agent_start", () => report("working"));
  pi.on("agent_settled", async (_event, ctx) => {
    if (ctx.isIdle()) await report("idle");
  });
}
