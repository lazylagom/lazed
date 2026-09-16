import { BellIcon, Cancel01Icon } from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import type { TerminalInfo } from "../shared/lazed";

function basename(p?: string) {
  if (!p) return "";
  const parts = p.replace(/\/$/, "").split("/");
  return parts[parts.length - 1] || p;
}

/** Attention queue: terminals whose agent wants a human. Blocked rows sort
 *  before done rows — a permission dialog needs action, a finished agent is
 *  just information. The panel is ephemeral by design: dismiss entries and
 *  they reappear only if that terminal goes back to work and blocks again. */
export function InboxPanel({
  items,
  onJump,
  onDismiss,
  onDismissAll,
  onClose,
}: {
  items: { term: TerminalInfo; project?: string }[];
  onJump: (t: TerminalInfo) => void;
  onDismiss: (termId: string) => void;
  onDismissAll: () => void;
  onClose: () => void;
}) {
  return (
    <div className="modal inbox">
      <div className="modal-head">
        <span className="modal-title">
          <HugeiconsIcon icon={BellIcon} size={13} strokeWidth={1.5} /> inbox
        </span>
        <div className="modal-head-actions">
          {items.length > 0 && (
            <button
              type="button"
              className="modal-btn"
              onClick={onDismissAll}
              title="dismiss all"
            >
              clear
            </button>
          )}
          <button
            type="button"
            className="modal-btn"
            onClick={onClose}
            title="close (esc)"
          >
            ✕
          </button>
        </div>
      </div>
      <div className="modal-body">
        {items.length === 0 && (
          <div className="modal-empty">no agents need attention</div>
        )}
        {items.map(({ term, project }) => (
          <div key={term.term_id} className={`inbox-row ${term.agent_status}`}>
            <button
              type="button"
              className="inbox-jump"
              onClick={() => onJump(term)}
              title="jump to terminal"
            >
              <span className={`dot ${term.agent_status}`} />
              <span className="inbox-agent">
                {term.agent_kind ?? basename(term.cwd) ?? term.term_id}
              </span>
              <span className="inbox-ws">{project}</span>
              <span className="inbox-status">{term.agent_status}</span>
            </button>
            <button
              type="button"
              className="inbox-dismiss"
              onClick={() => onDismiss(term.term_id)}
              title="dismiss"
            >
              <HugeiconsIcon icon={Cancel01Icon} size={12} strokeWidth={1.5} />
            </button>
          </div>
        ))}
      </div>
    </div>
  );
}
