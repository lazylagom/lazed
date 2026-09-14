import { useEffect, useRef, useState } from "react";

export function RemoteBar({
  connected,
  onConnect,
  onDisconnect,
  onClose,
}: {
  connected?: string;
  onConnect: (target: string) => void;
  onDisconnect: () => void;
  onClose: () => void;
}) {
  const [target, setTarget] = useState("");
  const [busy, setBusy] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  const connect = () => {
    const t = target.trim();
    if (!t || busy) return;
    setBusy(true);
    onConnect(t);
  };

  return (
    <div className="modal-overlay" onMouseDown={onClose}>
      <div
        className="modal"
        onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          if (e.key === "Escape") onClose();
          else if (e.key === "Enter") connect();
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
            <input
              ref={inputRef}
              className="modal-input"
              placeholder="user@host — remote herdr machine"
              value={target}
              onChange={(e) => setTarget(e.target.value)}
            />
            <button
              type="button"
              className="fanout-go"
              disabled={busy || !target.trim()}
              onClick={connect}
            >
              {busy ? "connecting…" : "attach over ssh"}
            </button>
          </>
        )}
      </div>
    </div>
  );
}
