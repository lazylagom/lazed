import { useRef, useState } from "react";
import { Modal } from "../components/Modal";
import { AGENT_KINDS } from "../shared/lazed";

export function AgentPicker({
  onPick,
  onClose,
}: {
  onPick: (kind: string) => void;
  onClose: () => void;
}) {
  const [filter, setFilter] = useState("");
  const [index, setIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);

  const kinds = AGENT_KINDS.filter((k) =>
    k.toLowerCase().includes(filter.toLowerCase()),
  );

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") onClose();
    else if (e.key === "ArrowDown") {
      e.preventDefault();
      setIndex((i) => Math.min(i + 1, kinds.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setIndex((i) => Math.max(i - 1, 0));
    } else if (e.key === "Enter" && kinds[index]) {
      onPick(kinds[index]);
    }
  };

  return (
    <Modal onClose={onClose} onKeyDown={onKey} initialFocusRef={inputRef}>
      <input
        ref={inputRef}
        className="modal-input"
        placeholder="start agent in focused term…"
        value={filter}
        onChange={(e) => {
          setFilter(e.target.value);
          setIndex(0);
        }}
      />
      <div className="modal-list">
        {kinds.map((k, i) => (
          <button
            key={k}
            type="button"
            className={`modal-item ${i === index ? "sel" : ""}`}
            onMouseEnter={() => setIndex(i)}
            onClick={() => onPick(k)}
          >
            {k}
          </button>
        ))}
        {kinds.length === 0 && <div className="modal-empty">no match</div>}
      </div>
    </Modal>
  );
}
