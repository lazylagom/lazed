import {
  ArrowDown01Icon,
  GitBranchIcon,
  MultiplicationSignIcon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { useEffect, useRef, useState } from "react";
import { Modal } from "../components/Modal";
import { lazed } from "../shared/lazed";

/** What the caller needs to run herdr `worktree.create` (git worktree add). */
export interface NewWorktreeRequest {
  branch: string;
  base?: string;
  label?: string;
}

interface RepoBranch {
  name: string;
  checked_out: boolean;
}

/** Base ref input with a pick list of the repo's local branches — typing
 * filters the list, ↑/↓ + Enter pick, Esc closes just the list. Free text
 * stays allowed: any ref (tag, sha) is a valid base. */
function BaseField({
  value,
  branches,
  onChange,
}: {
  value: string;
  branches: RepoBranch[];
  onChange: (v: string) => void;
}) {
  const [listOpen, setListOpen] = useState(false);
  const [hl, setHl] = useState(-1);
  const q = value.trim().toLowerCase();
  const filtered = branches.filter((b) => b.name.toLowerCase().includes(q));

  const pick = (b: RepoBranch) => {
    onChange(b.name);
    setListOpen(false);
    setHl(-1);
  };

  return (
    <div className="newwt-combo">
      <label className="newwt-field">
        <span className="newwt-label">Base</span>
        <input
          className="newwt-input"
          placeholder="current HEAD"
          value={value}
          spellCheck={false}
          onChange={(e) => {
            onChange(e.target.value);
            setListOpen(true);
            setHl(-1);
          }}
          onFocus={() => setListOpen(true)}
          onBlur={() => setListOpen(false)}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown" || e.key === "ArrowUp") {
              e.preventDefault();
              e.stopPropagation();
              if (!listOpen) {
                setListOpen(true);
                return;
              }
              const n = filtered.length;
              if (!n) return;
              setHl((h) =>
                e.key === "ArrowDown" ? (h + 1) % n : (h - 1 + n) % n,
              );
            } else if (e.key === "Enter" && listOpen && filtered[hl]) {
              e.preventDefault();
              e.stopPropagation();
              pick(filtered[hl]);
            } else if (e.key === "Escape" && listOpen) {
              e.stopPropagation();
              setListOpen(false);
            }
          }}
        />
        {branches.length > 0 && (
          <button
            type="button"
            className="newwt-caret"
            tabIndex={-1}
            aria-label="list branches"
            onMouseDown={(e) => {
              e.preventDefault();
              setListOpen((o) => !o);
            }}
          >
            <HugeiconsIcon icon={ArrowDown01Icon} size={13} strokeWidth={1.5} />
          </button>
        )}
      </label>
      {listOpen && filtered.length > 0 && (
        <div className="newwt-branch-list">
          {filtered.map((b, i) => (
            <button
              key={b.name}
              type="button"
              ref={
                i === hl
                  ? (el) => el?.scrollIntoView({ block: "nearest" })
                  : undefined
              }
              className={`newwt-branch${i === hl ? " hl" : ""}`}
              onMouseEnter={() => setHl(i)}
              onMouseDown={(e) => {
                e.preventDefault();
                pick(b);
              }}
            >
              <span className="newwt-branch-name">{b.name}</span>
              {b.checked_out && (
                <span className="newwt-branch-tag">checked out</span>
              )}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * ⌘N on the selected project — herdr `worktree.create` checks out a new
 * branch in a git worktree and opens it as a workspace with its own pane.
 * Branch is the only required field; base picks the fork point from the
 * repo's local branches, an empty base means "fork from the current HEAD".
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
  const [base, setBase] = useState("develop");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [branches, setBranches] = useState<RepoBranch[]>([]);
  const branchRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    let live = true;
    lazed
      .repoBranches(repoRoot)
      .then((r) => {
        if (live) setBranches(r.branches ?? []);
      })
      .catch(() => {}); // picker degrades to a plain input
    return () => {
      live = false;
    };
  }, [repoRoot]);

  const trimmed = branch.trim();
  const taken = branches.some((b) => b.name === trimmed);
  // git refuses these outright — catch them before the daemon round-trip
  const invalid =
    /\s|\.\.|^[-/]|[/]$|[~^:?*[\\]|@\{/.test(trimmed) ||
    trimmed.endsWith(".lock");
  const canSubmit = Boolean(trimmed) && !invalid && !taken && !busy;

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
        <BaseField value={base} branches={branches} onChange={setBase} />
        <label className="newwt-field">
          <span className="newwt-label">New branch</span>
          <input
            ref={branchRef}
            className="newwt-input newwt-name"
            placeholder="feature/my-change"
            value={branch}
            spellCheck={false}
            onChange={(e) => setBranch(e.target.value)}
          />
        </label>
      </div>

      <div className="newwt-note">
        {taken
          ? `A branch named "${trimmed}" already exists.`
          : invalid && trimmed
            ? "Not a valid branch name."
            : "A new git worktree opened as its own workspace. Uncommitted changes are not copied."}
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
