import { useEffect, useRef, useState } from "react";
import { type MachineInfo, herdr } from "../shared/herdr";

export function RemoteBar({
  connected,
  onConnect,
  onDisconnect,
  onClose,
}: {
  connected?: string;
  onConnect: (target: string, session?: string) => void;
  onDisconnect: () => void;
  onClose: () => void;
}) {
  const [target, setTarget] = useState("");
  const [busy, setBusy] = useState(false);
  const [machines, setMachines] = useState<MachineInfo[]>([]);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
    if (!connected) {
      // saved machines are local client state — load even while unattached
      herdr
        .machineList()
        .then(setMachines)
        .catch(() => setMachines([]));
    }
  }, [connected]);

  const connect = (t: string, session?: string) => {
    const trimmed = t.trim();
    if (!trimmed || busy) return;
    setBusy(true);
    onConnect(trimmed, session);
  };

  const connectMachine = (m: MachineInfo) =>
    connect(m.target, m.session === "default" ? undefined : m.session);

  // an attach can't be cancelled once spawned — don't let Escape/overlay
  // pretend otherwise (the modal closes itself when the result arrives)
  const tryClose = () => {
    if (!busy) onClose();
  };

  return (
    <div className="modal-overlay" onMouseDown={tryClose}>
      <div
        className="modal"
        onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          if (e.key === "Escape") tryClose();
          else if (e.key === "Enter") connect(target);
        }}
      >
        {connected ? (
          <>
            <div className="modal-empty">attached to {connected}</div>
            <button type="button" className="fanout-go" onClick={onDisconnect}>
              disconnect → local
            </button>
          </>
        ) : (
          <>
            {machines.length > 0 && (
              <div className="modal-list">
                {machines.map((m) => (
                  <button
                    key={m.id}
                    type="button"
                    className="modal-item remote-machine"
                    disabled={busy}
                    onClick={() => connectMachine(m)}
                  >
                    <span className="remote-machine-label">{m.label}</span>
                    <span className="remote-machine-target">
                      {m.target}
                      {m.session !== "default" ? ` · ${m.session}` : ""}
                      {m.enabled ? "" : " · disabled"}
                    </span>
                  </button>
                ))}
              </div>
            )}
            <input
              ref={inputRef}
              className="modal-input"
              placeholder="user@host — remote herdr machine"
              value={target}
              disabled={busy}
              onChange={(e) => setTarget(e.target.value)}
            />
            <button
              type="button"
              className="fanout-go"
              disabled={busy || !target.trim()}
              onClick={() => connect(target)}
            >
              {busy ? "connecting…" : "attach over ssh"}
            </button>
          </>
        )}
      </div>
    </div>
  );
}
