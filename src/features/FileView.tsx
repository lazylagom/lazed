import {
  ArrowUpRight01Icon,
  Cancel01Icon,
  Refresh01Icon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { deferred } from "../components/Deferred";
import { type FsGit, lazed } from "../shared/lazed";
import { parseDiff } from "./diff-parser";

const IS_MD = /\.(md|markdown|mdx)$/i;

type View = "auto" | "diff" | "text" | "preview";

const MdPreview = deferred<{ content: string }>(
  () => import("./MarkdownPreview"),
);

/** File surface inside the workspace body — Orca-style: a file click opens a
 * tab in the main area rather than a modal. Changed files lead with the diff,
 * markdown leads with a rendered preview; everything else is plain text. */
export function FileView({
  root,
  path,
  git,
  onClose,
}: {
  root: string;
  path: string;
  git?: FsGit | null;
  onClose: () => void;
}) {
  const [content, setContent] = useState<string | null>(null);
  const [diff, setDiff] = useState<string | null>(null);
  const [binary, setBinary] = useState(false);
  const [truncated, setTruncated] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [choice, setChoice] = useState<View>("auto");
  const seq = useRef(0);
  const abs = `${root.replace(/\/$/, "")}/${path}`;
  const isMd = IS_MD.test(path);

  const load = useCallback(() => {
    const my = ++seq.current;
    setContent(null);
    setDiff(null);
    setBinary(false);
    setTruncated(false);
    setErr(null);
    setChoice("auto");
    lazed
      .fsRead(abs)
      .then((r) => {
        if (seq.current !== my) return;
        setBinary(!!r.binary);
        setContent(r.content ?? "");
        setTruncated(!!r.truncated);
      })
      .catch((e) => seq.current === my && setErr(String(e)));
    if (git === "M" || git === "D") {
      lazed
        .fsDiff(root, path)
        .then((r) => seq.current === my && setDiff(r.diff || null))
        .catch(() => {});
    }
  }, [abs, root, path, git]);

  useEffect(() => {
    load();
    return () => {
      seq.current += 1;
    };
  }, [load]);

  // the view has no input to focus — Escape lives on the window, but the
  // files-panel search box owns its own Escape so inputs are excluded
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const tag = (e.target as HTMLElement)?.tagName;
      if (e.key === "Escape" && tag !== "INPUT" && tag !== "TEXTAREA")
        onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const files = useMemo(() => (diff ? parseDiff(diff) : []), [diff]);
  // auto: diff when the file has one, else md preview, else plain text
  const view: Exclude<View, "auto"> =
    choice === "auto"
      ? diff && !binary
        ? "diff"
        : isMd && content !== null
          ? "preview"
          : "text"
      : choice;

  return (
    <div className="file-view">
      <div className="fv-head">
        <span className="fv-path" title={abs}>
          {abs}
        </span>
        <span className="fv-views">
          {diff != null && !binary && (
            <button
              type="button"
              className={view === "diff" ? "sel" : ""}
              onClick={() => setChoice("diff")}
            >
              diff
            </button>
          )}
          {isMd && !binary && content !== null && (
            <button
              type="button"
              className={view === "preview" ? "sel" : ""}
              onClick={() => setChoice("preview")}
            >
              preview
            </button>
          )}
          <button
            type="button"
            className={view === "text" ? "sel" : ""}
            onClick={() => setChoice("text")}
          >
            text
          </button>
        </span>
        <button type="button" className="fv-btn" title="reload" onClick={load}>
          <HugeiconsIcon icon={Refresh01Icon} size={13} strokeWidth={1.5} />
        </button>
        <button
          type="button"
          className="fv-btn"
          title="open in default app"
          onClick={() => lazed.fsOpen(abs).catch(() => {})}
        >
          <HugeiconsIcon
            icon={ArrowUpRight01Icon}
            size={13}
            strokeWidth={1.5}
          />
        </button>
        <button
          type="button"
          className="fv-btn"
          title="close (esc)"
          onClick={onClose}
        >
          <HugeiconsIcon icon={Cancel01Icon} size={13} strokeWidth={1.5} />
        </button>
      </div>
      <div className="fv-body">
        {binary && (
          <div className="fv-empty">binary file — open externally</div>
        )}
        {/* a deleted file has a diff but no readable content — diff wins */}
        {!binary &&
          view === "diff" &&
          files.map((f) => (
            <div className="diff-file" key={f.path}>
              {f.lines.map((l, i) => (
                <div key={`${f.path}:${i}`} className={`diff-line ${l.kind}`}>
                  <span className="diff-lno">
                    {l.kind === "add" || l.kind === "ctx" ? l.newNo : ""}
                  </span>
                  <span className="diff-text">{l.text}</span>
                </div>
              ))}
            </div>
          ))}
        {!binary && view === "preview" && isMd && content !== null && (
          <MdPreview content={content} />
        )}
        {!binary && view === "text" && content !== null && (
          <pre className="fv-pre">{content}</pre>
        )}
        {!binary && content === null && view !== "diff" && (
          <div className="fv-empty">{err ?? "loading…"}</div>
        )}
        {!binary && view === "diff" && files.length === 0 && (
          <div className="fv-empty">{err ?? "no diff"}</div>
        )}
        {truncated && <div className="fv-note">file truncated at 512KB</div>}
      </div>
    </div>
  );
}
