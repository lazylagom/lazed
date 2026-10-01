import {
  Alert02Icon,
  MultiplicationSignIcon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { useRef, useSyncExternalStore } from "react";
import { Modal } from "./Modal";

export interface ConfirmOptions {
  title: string;
  message: string;
  /** optional monospace line under the message — a path or branch name */
  detail?: string;
  okLabel?: string;
  cancelLabel?: string;
  /** red confirm button for destructive actions */
  danger?: boolean;
}

interface Pending extends ConfirmOptions {
  resolve: (ok: boolean) => void;
}

let current: Pending | null = null;
const listeners = new Set<() => void>();
const emit = () => {
  for (const l of listeners) l();
};

/**
 * In-app replacement for the native `ask` dialog, themed like the rest of
 * lazed. Resolves true on confirm, false on cancel / Escape / backdrop.
 * Requires a mounted <ConfirmHost />; a second call while one is open
 * cancels the first.
 */
export function confirmDialog(opts: ConfirmOptions): Promise<boolean> {
  current?.resolve(false);
  return new Promise((resolve) => {
    current = { ...opts, resolve };
    emit();
  });
}

function settle(ok: boolean) {
  const p = current;
  current = null;
  emit();
  p?.resolve(ok);
}

function subscribe(l: () => void) {
  listeners.add(l);
  return () => {
    listeners.delete(l);
  };
}

export function ConfirmHost() {
  const p = useSyncExternalStore(subscribe, () => current);
  if (!p) return null;
  return <ConfirmModal key={p.title + p.message} p={p} />;
}

function ConfirmModal({ p }: { p: Pending }) {
  const okRef = useRef<HTMLButtonElement>(null);
  return (
    <Modal
      className="confirm"
      initialFocusRef={okRef}
      onClose={() => settle(false)}
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.preventDefault();
          settle(false);
        }
      }}
    >
      <div className="addproj-head">
        <span className={`confirm-ico ${p.danger ? "danger" : ""}`}>
          <HugeiconsIcon icon={Alert02Icon} size={18} strokeWidth={1.6} />
        </span>
        <span className="addproj-title">{p.title}</span>
        <button
          type="button"
          className="addproj-x"
          onClick={() => settle(false)}
          aria-label="close"
        >
          <HugeiconsIcon
            icon={MultiplicationSignIcon}
            size={18}
            strokeWidth={1.5}
          />
        </button>
      </div>
      <div className="confirm-msg">{p.message}</div>
      {p.detail && <div className="confirm-detail">{p.detail}</div>}
      <div className="confirm-actions">
        <button
          type="button"
          className="confirm-btn"
          onClick={() => settle(false)}
        >
          {p.cancelLabel ?? "Cancel"}
        </button>
        <button
          ref={okRef}
          type="button"
          className={`confirm-btn primary ${p.danger ? "danger" : ""}`}
          onClick={() => settle(true)}
        >
          {p.okLabel ?? "OK"}
        </button>
      </div>
    </Modal>
  );
}
