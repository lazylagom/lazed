import { useCallback, useEffect, useState } from "react";
import { type WorkspaceInfo, herdr } from "../shared/herdr";

interface DiffLine {
  kind: "ctx" | "add" | "del" | "hunk" | "meta";
  text: string;
  newNo?: number;
}

interface DiffFile {
  path: string;
  lines: DiffLine[];
}

interface Comment {
  file: string;
  line: number;
  text: string;
}

function parseDiff(diff: string): DiffFile[] {
  const files: DiffFile[] = [];
  let cur: DiffFile | null = null;
  let newNo = 0;
  for (const raw of diff.split("\n")) {
    if (raw.startsWith("diff --git")) {
      const m = / b\/(.+)$/.exec(raw);
      cur = { path: m?.[1] ?? "?", lines: [] };
      files.push(cur);
    } else if (cur) {
      if (raw.startsWith("@@")) {
        const m = /\+(\d+)/.exec(raw);
        newNo = m ? Number(m[1]) : 0;
        cur.lines.push({ kind: "hunk", text: raw });
      } else if (raw.startsWith("+")) {
        cur.lines.push({ kind: "add", text: raw, newNo: newNo++ });
      } else if (raw.startsWith("-")) {
        cur.lines.push({ kind: "del", text: raw });
      } else if (
        !raw.startsWith("index ") &&
        !raw.startsWith("---") &&
        !raw.startsWith("+++") &&
        !raw.startsWith("new file") &&
        !raw.startsWith("deleted file")
      ) {
        cur.lines.push({ kind: "ctx", text: raw, newNo: newNo++ });
      }
    }
  }
  return files;
}

export function DiffView({
  workspace,
  agentPaneId,
  onClose,
  onJump,
}: {
  workspace: WorkspaceInfo;
  agentPaneId?: string;
  onClose: () => void;
  onJump: () => void;
}) {
  const wt = workspace.worktree;
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
  const [mergeMsg, setMergeMsg] = useState<string | null>(null);
  const [confirmMerge, setConfirmMerge] = useState(false);

  const load = useCallback(() => {
    if (!wt) return;
    setError(null);
    herdr
      .worktreeDiff(wt.checkout_path)
      .then(setData)
      .catch((e) => setError(String(e)));
  }, [wt]);

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
    if (!agentPaneId || comments.length === 0 || !data) return;
    const body = [
      `Review comments on branch ${data.branch}:`,
      "",
      ...comments.map((c) => `${c.file}:${c.line} — ${c.text}`),
      "",
      "Please address these and update the branch.",
    ].join("\n");
    herdr.agentPrompt(agentPaneId, body).catch((e) => setError(String(e)));
    setComments([]);
  };

  const doMerge = () => {
    if (!wt?.repo_root || !data) return;
    herdr
      .worktreeMerge(wt.repo_root, data.branch)
      .then((r) => setMergeMsg(r.output.trim() || (r.ok ? "merged" : "failed")))
      .catch((e) => setMergeMsg(String(e)));
    setConfirmMerge(false);
  };

  const files = data ? parseDiff(data.diff) : [];

  return (
    <div className="diff-panel">
      <div className="diff-head">
        <span className="diff-title">
          ⑂ {data?.branch ?? workspace.label}
          {data?.base ? ` ← ${data.base}` : ""}
        </span>
        <button type="button" onClick={load} title="refresh diff">
          ↻
        </button>
        <button type="button" onClick={onJump} title="jump to pane">
          ⇢ pane
        </button>
        {wt?.repo_root && data?.branch && (
          <button
            type="button"
            className="diff-merge"
            onClick={() => setConfirmMerge(true)}
            title={`merge ${data.branch} into ${wt.repo_root}`}
          >
            merge
          </button>
        )}
        <button type="button" className="side-close" onClick={onClose}>
          ✕
        </button>
      </div>
      {data?.stat && <div className="diff-stat">{data.stat.trim()}</div>}
      {(data?.untracked?.length ?? 0) > 0 && (
        <div className="diff-stat">untracked: {data?.untracked.join(", ")}</div>
      )}
      {mergeMsg && <div className="diff-stat">{mergeMsg}</div>}
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
            disabled={!agentPaneId}
            onClick={sendReview}
          >
            send {comments.length} comment{comments.length === 1 ? "" : "s"} to
            agent
          </button>
        </div>
      )}
      {confirmMerge && (
        <div className="diff-confirm">
          merge <b>{data?.branch}</b> into {wt?.repo_root}?
          <button type="button" onClick={doMerge}>
            yes, merge
          </button>
          <button type="button" onClick={() => setConfirmMerge(false)}>
            cancel
          </button>
        </div>
      )}
    </div>
  );
}
