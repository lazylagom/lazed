import { useRef, useState } from "react";
import { Modal } from "../components/Modal";

export type PromptTarget =
  | { kind: "focused"; termId: string }
  | { kind: "agents" }
  | { kind: "all" };

export function PromptBar({
  focusedTerm,
  agentCount,
  termCount,
  onSubmit,
  onClose,
}: {
  focusedTerm: string | null;
  agentCount: number;
  termCount: number;
  onSubmit: (text: string, target: PromptTarget) => void;
  onClose: () => void;
}) {
  const [text, setText] = useState("");
  const [target, setTarget] = useState<"focused" | "agents" | "all">("focused");
  const inputRef = useRef<HTMLInputElement>(null);

  const submit = () => {
    const t = text.trim();
    if (!t) return;
    if (target === "focused" && focusedTerm) {
      onSubmit(t, { kind: "focused", termId: focusedTerm });
    } else if (target === "agents") {
      onSubmit(t, { kind: "agents" });
    } else {
      onSubmit(t, { kind: "all" });
    }
    onClose();
  };

  return (
    <Modal
      onClose={onClose}
      initialFocusRef={inputRef}
      onKeyDown={(e) => {
        if (e.key === "Escape") onClose();
        else if (e.key === "Enter") submit();
      }}
    >
      <input
        ref={inputRef}
        className="modal-input"
        placeholder="prompt text…"
        value={text}
        onChange={(e) => setText(e.target.value)}
      />
      <div className="prompt-targets">
        <button
          type="button"
          className={target === "focused" ? "sel" : ""}
          onClick={() => setTarget("focused")}
        >
          focused term {focusedTerm ? `(${focusedTerm})` : "(none)"}
        </button>
        <button
          type="button"
          className={target === "agents" ? "sel" : ""}
          onClick={() => setTarget("agents")}
        >
          all agents ({agentCount})
        </button>
        <button
          type="button"
          className={target === "all" ? "sel" : ""}
          onClick={() => setTarget("all")}
        >
          all terms ({termCount})
        </button>
      </div>
    </Modal>
  );
}
