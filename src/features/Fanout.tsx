import { useEffect, useRef, useState } from "react";
import { AGENT_KINDS } from "../shared/lazed";

export interface FanoutRequest {
  repo: string;
  base?: string;
  prefix: string;
  kinds: string[];
  prompt: string;
}

const POPULAR = ["claude", "codex", "gemini", "opencode", "devin"];

export function Fanout({
  defaultRepo,
  onSubmit,
  onClose,
}: {
  defaultRepo: string;
  onSubmit: (req: FanoutRequest) => void;
  onClose: () => void;
}) {
  const [repo, setRepo] = useState(defaultRepo);
  const [base, setBase] = useState("");
  const [prefix, setPrefix] = useState("fanout");
  const [kinds, setKinds] = useState<string[]>(["claude"]);
  const [prompt, setPrompt] = useState("");
  const [busy, setBusy] = useState(false);
  const repoRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    repoRef.current?.focus();
    repoRef.current?.select();
  }, []);

  const toggle = (k: string) =>
    setKinds((ks) => (ks.includes(k) ? ks.filter((x) => x !== k) : [...ks, k]));

  const submit = () => {
    if (!repo.trim() || kinds.length === 0 || busy) return;
    setBusy(true);
    onSubmit({
      repo: repo.trim(),
      base: base.trim() || undefined,
      prefix: prefix.trim() || "fanout",
      kinds,
      prompt: prompt.trim(),
    });
  };

  const ordered = [
    ...POPULAR,
    ...AGENT_KINDS.filter((k) => !POPULAR.includes(k)),
  ];

  return (
    <div className="modal-overlay" onMouseDown={onClose}>
      <div
        className="modal fanout"
        onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          if (e.key === "Escape") onClose();
        }}
      >
        <input
          ref={repoRef}
          className="modal-input"
          placeholder="repo path…"
          value={repo}
          onChange={(e) => setRepo(e.target.value)}
        />
        <div className="fanout-row">
          <input
            className="modal-input"
            placeholder="base ref (auto)"
            value={base}
            onChange={(e) => setBase(e.target.value)}
          />
          <input
            className="modal-input"
            placeholder="branch prefix"
            value={prefix}
            onChange={(e) => setPrefix(e.target.value)}
          />
        </div>
        <div className="fanout-kinds">
          {ordered.map((k) => (
            <button
              key={k}
              type="button"
              className={`fanout-kind ${kinds.includes(k) ? "sel" : ""}`}
              onClick={() => toggle(k)}
            >
              {k}
            </button>
          ))}
        </div>
        <textarea
          className="modal-input fanout-prompt"
          placeholder="prompt for all selected agents…"
          value={prompt}
          onChange={(e) => setPrompt(e.target.value)}
          rows={3}
        />
        <button
          type="button"
          className="fanout-go"
          disabled={busy || !repo.trim() || kinds.length === 0}
          onClick={submit}
        >
          {busy
            ? "creating…"
            : `fan out → ${kinds.length} worktree${kinds.length === 1 ? "" : "s"}`}
        </button>
      </div>
    </div>
  );
}
