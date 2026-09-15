import { useCallback, useEffect, useRef, useState } from "react";
import { type MachineInfo, herdr, shQuote } from "../shared/herdr";

export function RemoteBar({
  connected,
  focusedPane,
  onConnect,
  onDisconnect,
  onClose,
}: {
  connected?: string;
  /** pane that runs `herdr machine add` (it prompts interactively) */
  focusedPane?: string | null;
  onConnect: (target: string, session?: string) => void;
  onDisconnect: () => void;
  onClose: () => void;
}) {
  const [target, setTarget] = useState("");
  const [busy, setBusy] = useState(false);
  const [machines, setMachines] = useState<MachineInfo[]>([]);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [renameLabel, setRenameLabel] = useState("");
  const [removing, setRemoving] = useState<string | null>(null);
  const [addTarget, setAddTarget] = useState("");
  const [addLabel, setAddLabel] = useState("");
  const [actionMsg, setActionMsg] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  const reload = useCallback(() => {
    // saved machines are local client state — load even while unattached
    herdr
      .machineList()
      .then(setMachines)
      .catch(() => setMachines([]));
  }, []);

  useEffect(() => {
    inputRef.current?.focus();
    if (!connected) reload();
  }, [connected, reload]);

  const connect = (t: string, session?: string) => {
    const trimmed = t.trim();
    if (!trimmed || busy) return;
    setBusy(true);
    onConnect(trimmed, session);
  };

  const connectMachine = (m: MachineInfo) =>
    connect(m.target, m.session === "default" ? undefined : m.session);

  const doRename = (m: MachineInfo) => {
    const label = renameLabel.trim();
    setRenaming(null);
    if (!label || label === m.label) return;
    herdr
      .machineRename(m.id, label)
      .then(reload)
      .catch((e) => setActionMsg(`rename failed: ${e}`));
  };

  const doRemove = (m: MachineInfo) => {
    setRemoving(null);
    herdr
      .machineRemove(m.id)
      .then(reload)
      .catch((e) => setActionMsg(`remove failed: ${e}`));
  };

  // `herdr machine add` prompts to approve remote install/server replacement —
  // it can't run headless, so run it in the focused pane for the user to
  // approve there.
  const addViaPane = () => {
    const t = addTarget.trim();
    if (!focusedPane || !t) return;
    const cmd = `herdr machine add ${shQuote(t)}${addLabel.trim() ? ` --label ${shQuote(addLabel.trim())}` : ""}`;
    herdr
      .runInPane(focusedPane, cmd)
      .then(() => {
        setActionMsg(`running in ${focusedPane} — approve it there, then ↻`);
        setAddTarget("");
        setAddLabel("");
      })
      .catch((e) => setActionMsg(`add failed: ${e}`));
  };

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
                {machines.map((m) =>
                  renaming === m.id ? (
                    <div key={m.id} className="remote-machine-edit">
                      <input
                        className="modal-input"
                        value={renameLabel}
                        onChange={(e) => setRenameLabel(e.target.value)}
                        onKeyDown={(e) => {
                          if (e.key === "Enter") doRename(m);
                          if (e.key === "Escape") setRenaming(null);
                        }}
                      />
                      <button type="button" onClick={() => doRename(m)}>
                        save
                      </button>
                    </div>
                  ) : removing === m.id ? (
                    <div key={m.id} className="remote-machine-edit">
                      <span>remove {m.label}?</span>
                      <button type="button" onClick={() => doRemove(m)}>
                        yes
                      </button>
                      <button type="button" onClick={() => setRemoving(null)}>
                        no
                      </button>
                    </div>
                  ) : (
                    <div key={m.id} className="remote-machine-row">
                      <button
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
                      <button
                        type="button"
                        className="remote-machine-act"
                        title="rename machine"
                        onClick={() => {
                          setRenaming(m.id);
                          setRenameLabel(m.label);
                          setRemoving(null);
                        }}
                      >
                        ✎
                      </button>
                      <button
                        type="button"
                        className="remote-machine-act"
                        title="remove machine"
                        onClick={() => {
                          setRemoving(m.id);
                          setRenaming(null);
                        }}
                      >
                        ✕
                      </button>
                    </div>
                  ),
                )}
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
            <div className="remote-add">
              <input
                className="modal-input"
                placeholder="add machine: user@host"
                value={addTarget}
                onChange={(e) => setAddTarget(e.target.value)}
              />
              <input
                className="modal-input"
                placeholder="label (optional)"
                value={addLabel}
                onChange={(e) => setAddLabel(e.target.value)}
              />
              <div className="remote-add-row">
                <button
                  type="button"
                  disabled={!focusedPane || !addTarget.trim()}
                  title={
                    focusedPane
                      ? `run 'herdr machine add' in ${focusedPane}`
                      : "focus a shell pane first"
                  }
                  onClick={addViaPane}
                >
                  + add via pane {focusedPane ? `(${focusedPane})` : "(none)"}
                </button>
                <button
                  type="button"
                  title="refresh machine list"
                  onClick={reload}
                >
                  ↻
                </button>
              </div>
            </div>
            {actionMsg && <div className="modal-empty">{actionMsg}</div>}
          </>
        )}
      </div>
    </div>
  );
}
