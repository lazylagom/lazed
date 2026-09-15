import type { PaneInfo } from "../shared/herdr";

export function InboxButton({
  blocked,
  done,
  onToggle,
}: {
  blocked: number;
  done: number;
  onToggle: () => void;
}) {
  return (
    <button
      type="button"
      className={`inbox-btn ${blocked > 0 ? "alert" : ""}`}
      onClick={onToggle}
      title="agents needing attention"
    >
      inbox{blocked > 0 ? ` ${blocked}` : done > 0 ? ` ·${done}` : ""}
    </button>
  );
}

export function InboxPanel({
  items,
  onJump,
  onDismiss,
  onDismissAll,
  onClose,
}: {
  items: { pane: PaneInfo; workspace?: string; tab?: string }[];
  onJump: (pane: PaneInfo) => void;
  onDismiss: (paneId: string) => void;
  onDismissAll: () => void;
  onClose: () => void;
}) {
  return (
    <div className="inbox-panel">
      <div className="inbox-head">
        <span>needs attention</span>
        <span>
          {items.length > 0 && (
            <button
              type="button"
              className="inbox-clear"
              onClick={onDismissAll}
            >
              clear all
            </button>
          )}
          <button type="button" className="side-close" onClick={onClose}>
            ✕
          </button>
        </span>
      </div>
      {items.length === 0 ? (
        <div className="inbox-empty">all clear</div>
      ) : (
        items.map(({ pane, workspace, tab }) => (
          <div key={pane.pane_id} className="inbox-item-row">
            <button
              type="button"
              className="inbox-item"
              onClick={() => onJump(pane)}
            >
              <span className={`dot ${pane.agent_status}`} />
              <span className="inbox-name">
                {pane.display_agent ?? pane.agent ?? pane.pane_id}
              </span>
              <span className="inbox-where">
                {workspace}
                {tab ? ` › ${tab}` : ""}
              </span>
              <span className={`badge ${pane.agent_status}`}>
                {pane.agent_status}
              </span>
            </button>
            <button
              type="button"
              className="inbox-dismiss"
              title="dismiss"
              onClick={() => onDismiss(pane.pane_id)}
            >
              ✕
            </button>
          </div>
        ))
      )}
    </div>
  );
}
