import { ask } from "@tauri-apps/plugin-dialog";
import { useEffect, useRef, useState } from "react";
import { type SessionStatus, lazed } from "../shared/lazed";

/** Global build notice; a done/idle agent still owns a live terminal. */
export function DaemonUpdate() {
  const [status, setStatus] = useState<SessionStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const pending = useRef(false);
  const attempted = useRef<number | undefined>(undefined);

  useEffect(() => {
    let cancelled = false;
    const refresh = async () => {
      if (pending.current) return;
      pending.current = true;
      try {
        const next = await lazed.status();
        if (cancelled) return;
        setStatus(next);
        if (
          next?.binary_updated &&
          next.terms === 0 &&
          next.pid !== undefined &&
          attempted.current !== next.pid &&
          next.capabilities?.includes("server.stop_if_empty.v1")
        ) {
          attempted.current = next.pid;
          setBusy(true);
          setError(null);
          const restarted = await lazed.restartServer(true);
          if (!cancelled) setStatus(restarted);
        }
      } catch (e) {
        if (!cancelled) setError(String(e));
      } finally {
        pending.current = false;
        if (!cancelled) setBusy(false);
      }
    };
    void refresh();
    const timer = window.setInterval(refresh, 5_000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, []);

  const restart = async () => {
    if (pending.current) return;
    pending.current = true;
    setBusy(true);
    try {
      const current = await lazed.status();
      const ok = await ask(
        `Restart the lazed daemon to apply the new build? All processes in its ${current.terms ?? "open"} terminal(s), including agents, will be terminated. Panes will be restored, but running processes will not.`,
        { title: "Restart daemon", kind: "warning", okLabel: "Restart" },
      );
      if (!ok) return;
      setError(null);
      setStatus(await lazed.restartServer());
    } catch (e) {
      setError(String(e));
    } finally {
      pending.current = false;
      setBusy(false);
    }
  };

  if (!status?.binary_updated) return null;
  return (
    <div className="install-banner" aria-live="polite">
      <span>
        Daemon update available. Restart to apply the new build.
        {(status.terms ?? 0) > 0 &&
          ` ${status.terms} terminal(s) will be interrupted.`}
      </span>
      {error && <span className="warn">{error}</span>}
      <span className="grow" />
      <button type="button" disabled={busy} onClick={restart}>
        {busy ? "Restarting…" : "Restart"}
      </button>
    </div>
  );
}
