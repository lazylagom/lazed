import {
  ArrowDown01Icon,
  ArrowRight01Icon,
  Cancel01Icon,
  File01Icon,
  Folder01Icon,
  GitBranchIcon,
  GitCompareIcon,
  Refresh01Icon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { listen } from "@tauri-apps/api/event";
import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  type FsEntry,
  type FsGit,
  type FsHit,
  type FsTree,
  lazed,
} from "../shared/lazed";
import { safeUnlisten } from "../shared/unlisten";
import { useSidebarWidth } from "../widgets/sidebar-width";
import { type TreeNode, buildTree, filterEntries } from "./files-tree";

const POLL_MS = 6_000;
const SEARCH_DEBOUNCE_MS = 160;

function basename(p?: string | null) {
  if (!p) return "";
  const parts = p.replace(/\/$/, "").split("/");
  return parts[parts.length - 1] || p;
}

/** row class for the git status tint — "!" keeps the existing `ignored`
 * styling, the rest tint name+icon via s-* rules */
function gitClass(git?: FsGit | null) {
  if (!git) return "";
  return git === "!" ? "ignored" : `s-${git}`;
}

function FileBadge({ git }: { git?: FsGit | null }) {
  if (!git) return null;
  return (
    <span className={`file-badge g-${git === "!" ? "ignored" : git}`}>
      {git}
    </span>
  );
}

/** One file row/dir row in the tree; dirs expand in place, files open in
 * the workspace's file view. */
function TreeRows({
  nodes,
  depth,
  expanded,
  onToggle,
  onOpen,
}: {
  nodes: TreeNode[];
  depth: number;
  expanded: ReadonlySet<string>;
  onToggle: (path: string) => void;
  onOpen: (path: string, git?: FsGit | null) => void;
}) {
  return (
    <>
      {nodes.map((n) => (
        <div key={n.path}>
          {n.kind === "dir" ? (
            <>
              <button
                type="button"
                className={`file-row dir ${gitClass(n.git)}`}
                style={{ paddingLeft: 6 + depth * 14 }}
                onClick={() => !n.collapsed && onToggle(n.path)}
                title={n.path}
              >
                {n.collapsed ? (
                  <span className="file-caret" />
                ) : (
                  <HugeiconsIcon
                    icon={
                      expanded.has(n.path) ? ArrowDown01Icon : ArrowRight01Icon
                    }
                    size={10}
                    strokeWidth={2}
                    className="file-caret"
                  />
                )}
                <HugeiconsIcon
                  icon={Folder01Icon}
                  size={13}
                  strokeWidth={1.5}
                  className="file-ico"
                />
                <span className="file-name">{n.name}</span>
                <FileBadge git={n.git} />
              </button>
              {expanded.has(n.path) && !n.collapsed && (
                <TreeRows
                  nodes={n.children}
                  depth={depth + 1}
                  expanded={expanded}
                  onToggle={onToggle}
                  onOpen={onOpen}
                />
              )}
            </>
          ) : (
            <button
              type="button"
              className={`file-row ${gitClass(n.git)}`}
              style={{ paddingLeft: 6 + depth * 14 + 16 }}
              onClick={() => onOpen(n.path, n.git)}
              title={n.path}
            >
              <HugeiconsIcon
                icon={File01Icon}
                size={12}
                strokeWidth={1.5}
                className="file-ico"
              />
              <span className="file-name">{n.name}</span>
              <FileBadge git={n.git} />
            </button>
          )}
        </div>
      ))}
    </>
  );
}

/** Right sidebar — file tree of the active checkout with git badges and
 * name/content search. Clicks delegate to the workspace's file view (the
 * main-area tab surface) via onOpenFile. Entirely local: herdr knows
 * nothing about it. */
export const FilesPanel = memo(function FilesPanel({
  root,
  label,
  branch,
  paneCount,
  onClose,
  onOpenDiff,
  onOpenFile,
}: {
  /** checkout path of the active workspace — undefined while none exists */
  root?: string;
  /** workspace display name for the header */
  label?: string;
  branch?: string;
  paneCount?: number;
  onClose: () => void;
  /** opens the workspace-wide DiffView overlay, when one exists */
  onOpenDiff?: () => void;
  /** opens a file in the main-area file view */
  onOpenFile: (path: string, git?: FsGit | null, root?: string) => void;
}) {
  const [tree, setTree] = useState<FsTree | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const [query, setQuery] = useState("");
  const [mode, setMode] = useState<"names" | "contents">("names");
  const [hits, setHits] = useState<FsHit[]>([]);
  const [hitsTruncated, setHitsTruncated] = useState(false);
  const [searching, setSearching] = useState(false);
  const reloadRef = useRef<() => void>(() => {});
  const { width, sideRef, onResizeDown, resetWidth } = useSidebarWidth({
    storageKey: "files-width",
    side: "right",
    defaultWidth: 320,
  });

  const load = useCallback(() => reloadRef.current(), []);

  useEffect(() => {
    let disposed = false;
    let inFlight = false;
    let queued = false;
    let watchId: string | undefined;
    let unlisten: (() => void) | undefined;
    let poll: number | undefined;
    let debounce: number | undefined;
    let signature = "";
    const run = () => {
      if (!root || disposed) return;
      if (inFlight) {
        queued = true;
        return;
      }
      inFlight = true;
      lazed
        .fsTree(root)
        .then((t) => {
          if (disposed) return;
          setErr(null);
          const next = JSON.stringify(t);
          if (next !== signature) {
            signature = next;
            setTree(t);
          }
        })
        .catch((e) => {
          if (!disposed) setErr(String(e));
        })
        .finally(() => {
          inFlight = false;
          if (!disposed && queued) {
            queued = false;
            run();
          }
        });
    };
    reloadRef.current = run;
    setTree(null);
    setErr(null);
    setExpanded(new Set());
    setQuery("");
    setHits([]);
    run();
    const fallback = () => {
      if (!disposed && poll === undefined)
        poll = window.setInterval(run, POLL_MS);
    };
    if (root) {
      (async () => {
        const stop = await listen<{ watch_id: string; error?: string }>(
          "fs.changed",
          ({ payload }) => {
            if (disposed || payload.watch_id !== watchId) return;
            if (payload.error) fallback();
            if (debounce !== undefined) window.clearTimeout(debounce);
            debounce = window.setTimeout(run, 250);
          },
        );
        if (disposed) {
          safeUnlisten(stop);
          return;
        }
        unlisten = stop;
        const watch = await lazed.fsWatch(root);
        if (disposed) {
          await lazed.fsUnwatch(watch.watch_id);
          return;
        }
        watchId = watch.watch_id;
        run(); // close the gap between the first tree and watcher setup
      })().catch(fallback);
    }
    return () => {
      disposed = true;
      reloadRef.current = () => {};
      if (poll !== undefined) window.clearInterval(poll);
      if (debounce !== undefined) window.clearTimeout(debounce);
      if (unlisten) safeUnlisten(unlisten);
      if (watchId) lazed.fsUnwatch(watchId).catch(() => {});
    };
  }, [root]);

  const open = useCallback(
    (path: string, git?: FsGit | null) =>
      onOpenFile(path, git, tree?.root ?? root),
    [onOpenFile, tree?.root, root],
  );

  // debounced search — names filter client-side, contents hit the backend
  useEffect(() => {
    let disposed = false;
    setHits([]);
    setHitsTruncated(false);
    if (!root || !query.trim() || mode === "names") {
      setSearching(false);
      return;
    }
    setSearching(true);
    const t = window.setTimeout(() => {
      lazed
        .fsSearch(tree?.root ?? root, query.trim())
        .then((r) => {
          if (disposed) return;
          setErr(null);
          setHits(r.results);
          setHitsTruncated(!!r.truncated);
        })
        .catch((e) => {
          if (!disposed) setErr(String(e));
        })
        .finally(() => {
          if (!disposed) setSearching(false);
        });
    }, SEARCH_DEBOUNCE_MS);
    return () => {
      disposed = true;
      window.clearTimeout(t);
    };
  }, [root, tree?.root, query, mode]);

  const nodes = useMemo(() => buildTree(tree?.entries ?? []), [tree]);
  const nameHits = useMemo(
    () =>
      mode === "names" && query.trim()
        ? filterEntries(tree?.entries ?? [], query)
        : [],
    [mode, query, tree],
  );
  const changedCount = useMemo(
    () =>
      (tree?.entries ?? []).filter(
        (e) => e.git === "M" || e.git === "D" || e.git === "U",
      ).length,
    [tree],
  );

  const toggle = useCallback(
    (path: string) =>
      setExpanded((prev) => {
        const next = new Set(prev);
        if (next.has(path)) next.delete(path);
        else next.add(path);
        return next;
      }),
    [],
  );

  const gitByPath = useMemo(
    () => new Map((tree?.entries ?? []).map((e) => [e.path, e.git])),
    [tree],
  );

  const searchingActive = query.trim().length > 0;

  return (
    <div ref={sideRef} className="files-panel" style={{ width }}>
      <div
        className="divider x files-resize"
        onMouseDown={onResizeDown}
        onDoubleClick={resetWidth}
        title="drag to resize · double-click to reset"
      />
      <div className="side-head">
        <span className="side-head-label" title={root}>
          {label ?? basename(root) ?? "FILES"}
        </span>
        <button
          type="button"
          className="side-head-btn"
          title="refresh"
          onClick={load}
        >
          <HugeiconsIcon icon={Refresh01Icon} size={15} strokeWidth={1.5} />
        </button>
        <button
          type="button"
          className="side-head-btn"
          title="close files (⇧⌘E)"
          onClick={onClose}
        >
          <HugeiconsIcon icon={Cancel01Icon} size={15} strokeWidth={1.5} />
        </button>
      </div>
      <div className="files-search">
        <input
          className="modal-input files-input"
          placeholder="Find files"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Escape") setQuery("");
          }}
        />
        <div className="files-tabs">
          <button
            type="button"
            className={mode === "names" ? "sel" : ""}
            onClick={() => setMode("names")}
          >
            Names
          </button>
          <button
            type="button"
            className={mode === "contents" ? "sel" : ""}
            onClick={() => setMode("contents")}
          >
            Contents
          </button>
        </div>
      </div>
      <div className="side-scroll files-scroll">
        {!root && <div className="files-empty">no checkout selected</div>}
        {root && err && <div className="files-empty">{err}</div>}
        {root && !err && !tree && <div className="files-empty">loading…</div>}
        {root && !err && tree && !searchingActive && (
          <TreeRows
            nodes={nodes}
            depth={0}
            expanded={expanded}
            onToggle={toggle}
            onOpen={open}
          />
        )}
        {root && tree && searchingActive && mode === "names" && (
          <>
            {nameHits.length === 0 && (
              <div className="files-empty">no matches</div>
            )}
            {nameHits.map((e: FsEntry) => (
              <button
                key={e.path}
                type="button"
                className={`file-row flat ${gitClass(e.git)}`}
                onClick={() => e.kind === "file" && open(e.path, e.git)}
                title={e.path}
              >
                <HugeiconsIcon
                  icon={e.kind === "dir" ? Folder01Icon : File01Icon}
                  size={12}
                  strokeWidth={1.5}
                  className="file-ico"
                />
                <span className="file-name">{e.path}</span>
                <FileBadge git={e.git} />
              </button>
            ))}
          </>
        )}
        {root && searchingActive && mode === "contents" && (
          <>
            {searching && <div className="files-empty">searching…</div>}
            {!searching && hits.length === 0 && (
              <div className="files-empty">no matches</div>
            )}
            {hits.map((h, i) => (
              <button
                key={`${h.path}:${h.line}:${i}`}
                type="button"
                className="file-row hit"
                onClick={() => open(h.path, gitByPath.get(h.path))}
                title={`${h.path}:${h.line}`}
              >
                <span className={`hit-loc ${gitClass(gitByPath.get(h.path))}`}>
                  {h.path}:{h.line}
                </span>
                <span className="hit-text">{h.text}</span>
              </button>
            ))}
            {hitsTruncated && (
              <div className="files-empty">more than 300 hits — refine</div>
            )}
          </>
        )}
      </div>
      <div className="files-foot">
        <span className="files-foot-item" title={branch ?? "no branch"}>
          <HugeiconsIcon icon={GitBranchIcon} size={11} strokeWidth={1.5} />
          {branch ?? "—"}
        </span>
        <span className="files-foot-item">{changedCount} changed</span>
        {paneCount !== undefined && (
          <span className="files-foot-item">{paneCount} panes</span>
        )}
        {onOpenDiff && (
          <button
            type="button"
            className="files-foot-btn"
            title="workspace diff"
            onClick={onOpenDiff}
          >
            <HugeiconsIcon icon={GitCompareIcon} size={11} strokeWidth={1.5} />
            diff
          </button>
        )}
      </div>
    </div>
  );
});
