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
  onClose,
}: {
  items: { pane: PaneInfo; workspace?: string; tab?: string }[];
  onJump: (pane: PaneInfo) => void;
  onClose: () => void;
}) {
  return (
    <div className="inbox-panel">
      <div className="inbox-head">
        <span>needs attention</span>
        <button type="button" className="side-close" onClick={onClose}>
          ✕
        </button>
      </div>
      {items.length === 0 ? (
        <div className="inbox-empty">all clear</div>
      ) : (
        items.map(({ pane, workspace, tab }) => (
          <button
            key={pane.pane_id}
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
        ))
      )}
    </div>
  );
}
