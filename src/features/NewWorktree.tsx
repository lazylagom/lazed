import {
  GitBranchIcon,
  MultiplicationSignIcon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { useRef, useState } from "react";
import { Modal } from "../components/Modal";

/** What the caller needs to run `workspace.create` (git worktree add). */
export interface NewWorktreeRequest {
  branch: string;
  base?: string;
  label?: string;
}

/**
 * ⌘N on the selected project — a git worktree for a new branch, checked out
 * under ~/.lazed/worktrees/<repo>/<branch-slug> with its own pane. Branch is
 * the only required field; an empty base means "fork from the current HEAD".
 */
export function NewWorktree({
  projectName,
  repoRoot,
  onSubmit,
  onClose,
}: {
  projectName: string;
  repoRoot: string;
  onSubmit: (req: NewWorktreeRequest) => Promise<void>;
  onClose: () => void;
}) {
  const [branch, setBranch] = useState("");
  const [base, setBase] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const branchRef = useRef<HTMLInputElement>(null);

  const trimmed = branch.trim();
  // git refuses these outright — catch them before the daemon round-trip
  const invalid =
    /\s|\.\.|^[-/]|[/]$|[~^:?*[\\]|@\{/.test(trimmed) ||
    trimmed.endsWith(".lock");
  const canSubmit = Boolean(trimmed) && !invalid && !busy;

  const submit = async () => {
    if (!canSubmit) return;
    setBusy(true);
    setError(null);
    try {
      await onSubmit({ branch: trimmed, base: base.trim() || undefined });
      onClose();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  return (
    <Modal
      onClose={onClose}
      className="newwt"
      initialFocusRef={branchRef}
      onKeyDown={(e) => {
        if (e.key === "Escape") onClose();
        if (e.key === "Enter" && canSubmit) {
          e.preventDefault();
          void submit();
        }
      }}
    >
      <div className="addproj-head">
        <span className="addproj-title">New worktree</span>
        <button
          type="button"
          className="addproj-x"
          onClick={onClose}
          aria-label="close"
        >
          <HugeiconsIcon
            icon={MultiplicationSignIcon}
            size={18}
            strokeWidth={1.5}
          />
        </button>
      </div>
      <div className="newwt-repo" title={repoRoot}>
        <HugeiconsIcon icon={GitBranchIcon} size={13} strokeWidth={1.5} />
        <span>{projectName}</span>
      </div>

      <div className="newwt-fields">
        <label className="newwt-field">
          <span className="newwt-label">Branch</span>
          <input
            ref={branchRef}
            className="newwt-input"
            placeholder="feature/my-change"
            value={branch}
            spellCheck={false}
            onChange={(e) => setBranch(e.target.value)}
          />
        </label>
        <label className="newwt-field">
          <span className="newwt-label">Base</span>
          <input
            className="newwt-input"
            placeholder="current HEAD"
            value={base}
            spellCheck={false}
            onChange={(e) => setBase(e.target.value)}
          />
        </label>
      </div>

      <div className="newwt-note">
        {invalid && trimmed
          ? "Not a valid branch name."
          : "A new checkout under ~/.lazed/worktrees with its own pane. Uncommitted changes are not copied."}
      </div>
      {error && <div className="newwt-error">{error}</div>}

      <button
        type="button"
        className="fanout-go newwt-go"
        disabled={!canSubmit}
        onClick={submit}
      >
        {busy ? "creating…" : "Create worktree"}
      </button>
    </Modal>
  );
}
