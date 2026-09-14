import { useEffect, useRef, useState } from "react";

export type PromptTarget =
  | { kind: "focused"; paneId: string }
  | { kind: "agents" }
  | { kind: "all" };

export function PromptBar({
  focusedPane,
  agentCount,
  paneCount,
  onSubmit,
  onClose,
}: {
  focusedPane: string | null;
  agentCount: number;
  paneCount: number;
  onSubmit: (text: string, target: PromptTarget) => void;
  onClose: () => void;
}) {
  const [text, setText] = useState("");
  const [target, setTarget] = useState<"focused" | "agents" | "all">("focused");
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  const submit = () => {
    const t = text.trim();
    if (!t) return;
    if (target === "focused" && focusedPane) {
      onSubmit(t, { kind: "focused", paneId: focusedPane });
    } else if (target === "agents") {
      onSubmit(t, { kind: "agents" });
    } else {
      onSubmit(t, { kind: "all" });
    }
    onClose();
  };

  return (
    <div className="modal-overlay" onMouseDown={onClose}>
      <div
        className="modal"
        onMouseDown={(e) => e.stopPropagation()}
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
            focused pane {focusedPane ? `(${focusedPane})` : "(none)"}
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
            all panes ({paneCount})
          </button>
        </div>
      </div>
    </div>
  );
}
