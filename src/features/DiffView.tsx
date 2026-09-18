import { useCallback, useEffect, useMemo, useState } from "react";
import { lazed } from "../shared/lazed";
import { parseDiff } from "./diff-parser";
export { parseDiff } from "./diff-parser";

interface Comment {
  file: string;
  line: number;
  text: string;
}

export function DiffView({
  checkout,
  repoRoot,
  label,
  termId,
  agentTermId,
  onClose,
  onJump,
}: {
  checkout: string;
  repoRoot?: string;
  label?: string;
  termId: string;
  agentTermId?: string;
  onClose: () => void;
  onJump: () => void;
}) {
  const [data, setData] = useState<{
    branch: string;
    base?: string;
    diff: string;
    stat: string;
    untracked: string[];
  } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [comments, setComments] = useState<Comment[]>([]);
  const [draftFor, setDraftFor] = useState<{
    file: string;
    line: number;
  } | null>(null);
  const [draft, setDraft] = useState("");
  const [mergeRes, setMergeRes] = useState<{
    ok: boolean;
    output: string;
  } | null>(null);
  const [confirmMerge, setConfirmMerge] = useState(false);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const [removeErr, setRemoveErr] = useState<string | null>(null);

  const load = useCallback(() => {
    setError(null);
    lazed
      .worktreeDiff(checkout)
      .then(setData)
      .catch((e) => setError(String(e)));
  }, [checkout]);

  useEffect(() => {
    load();
  }, [load]);

  const addComment = () => {
    if (!draftFor || !draft.trim()) return;
    setComments((cs) => [
      ...cs,
      { file: draftFor.file, line: draftFor.line, text: draft.trim() },
    ]);
    setDraftFor(null);
    setDraft("");
  };

  const sendReview = () => {
    if (!agentTermId || comments.length === 0 || !data) return;
    const body = [
      `Review comments on branch ${data.branch}:`,
      "",
      ...comments.map((c) => `${c.file}:${c.line} — ${c.text}`),
      "",
      "Please address these and update the branch.",
    ].join("\n");
    lazed.agentPrompt(agentTermId, body).catch((e) => setError(String(e)));
    setComments([]);
  };

  const doMerge = () => {
    if (!repoRoot || !data) return;
    lazed
      .worktreeMerge(repoRoot, data.branch)
      .then(setMergeRes)
      .catch((e) => setMergeRes({ ok: false, output: String(e) }));
    setConfirmMerge(false);
  };

  const doRemove = (force: boolean) => {
    setConfirmRemove(false);
    lazed
      .worktreeRemove(termId, force)
      .then(() => onClose())
      .catch((e) => setRemoveErr(String(e)));
  };

  const diff = data?.diff;
  const files = useMemo(
    () => (diff === undefined ? [] : parseDiff(diff)),
    [diff],
  );

  return (
    <div className="diff-panel">
      <div className="diff-head">
        <span className="diff-title">
          ⑂ {data?.branch ?? label ?? checkout}
          {data?.base ? ` ← ${data.base}` : ""}
        </span>
        <button type="button" onClick={load} title="refresh diff">
          ↻
        </button>
        <button type="button" onClick={onJump} title="jump to terminal">
          ⇢ term
        </button>
        {repoRoot && data?.branch && (
          <button
            type="button"
            className="diff-merge"
            onClick={() => setConfirmMerge(true)}
            title={`merge ${data.branch} into ${repoRoot}`}
          >
            merge
          </button>
        )}
        <button
          type="button"
          className="diff-merge"
          onClick={() => setConfirmRemove(true)}
          title="remove this worktree and its terminal"
        >
          remove
        </button>
        <button type="button" className="side-close" onClick={onClose}>
          ✕
        </button>
      </div>
      {data?.stat && <div className="diff-stat">{data.stat.trim()}</div>}
      {(data?.untracked?.length ?? 0) > 0 && (
        <div className="diff-stat">untracked: {data?.untracked.join(", ")}</div>
      )}
      {mergeRes && (
        <div className="diff-stat">
          {mergeRes.output.trim() || (mergeRes.ok ? "merged" : "failed")}
          {mergeRes.ok && (
            <button
              type="button"
              className="diff-merge"
              onClick={() => setConfirmRemove(true)}
            >
              remove worktree?
            </button>
          )}
        </div>
      )}
      {removeErr && <div className="diff-stat">remove failed: {removeErr}</div>}
      {error && <div className="diff-stat">error: {error}</div>}
      <div className="diff-body">
        {files.length === 0 && !error && (
          <div className="inbox-empty">no changes yet</div>
        )}
        {files.map((f) => (
          <div key={f.path} className="diff-file">
            <div className="diff-fname">{f.path}</div>
            {f.lines.map((l, i) => (
              <button
                key={`${f.path}:${i}`}
                type="button"
                className={`diff-line ${l.kind}`}
                onClick={() => {
                  if (l.kind === "add" || l.kind === "ctx") {
                    setDraftFor({ file: f.path, line: l.newNo ?? 0 });
                    setDraft("");
                  }
                }}
              >
                <span className="diff-lno">
                  {l.kind === "add" || l.kind === "ctx" ? l.newNo : ""}
                </span>
                <span className="diff-text">{l.text}</span>
              </button>
            ))}
          </div>
        ))}
      </div>
      {draftFor && (
        <div className="diff-draft">
          <span>
            {draftFor.file}:{draftFor.line}
          </span>
          <input
            className="modal-input"
            placeholder="comment…"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") addComment();
              if (e.key === "Escape") setDraftFor(null);
            }}
          />
          <button type="button" onClick={addComment}>
            add
          </button>
        </div>
      )}
      {comments.length > 0 && (
        <div className="diff-comments">
          {comments.map((c, i) => (
            <div key={`${c.file}:${c.line}:${i}`} className="diff-comment">
              {c.file}:{c.line} — {c.text}
              <button
                type="button"
                onClick={() =>
                  setComments((cs) => cs.filter((_, j) => j !== i))
                }
              >
                ✕
              </button>
            </div>
          ))}
          <button
            type="button"
            className="fanout-go"
            disabled={!agentTermId}
            onClick={sendReview}
          >
            send {comments.length} comment{comments.length === 1 ? "" : "s"} to
            agent
          </button>
        </div>
      )}
      {confirmMerge && (
        <div className="diff-confirm">
          merge <b>{data?.branch}</b> into {repoRoot}?
          <button type="button" onClick={doMerge}>
            yes, merge
          </button>
          <button type="button" onClick={() => setConfirmMerge(false)}>
            cancel
          </button>
        </div>
      )}
      {confirmRemove && (
        <div className="diff-confirm">
          remove worktree <b>{checkout}</b> and its terminal?
          <button type="button" onClick={() => doRemove(false)}>
            yes, remove
          </button>
          <button type="button" onClick={() => doRemove(true)}>
            force
          </button>
          <button type="button" onClick={() => setConfirmRemove(false)}>
            cancel
          </button>
        </div>
      )}
    </div>
  );
}
