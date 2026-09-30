import {
  Cancel01Icon,
  CheckmarkSquare02Icon,
  PlusSignIcon,
  SquareIcon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { useCallback, useEffect, useRef, useState } from "react";
import { type TodoItem, todo } from "../shared/todo";

function relTime(at: number): string {
  const s = Math.max(0, Math.floor(Date.now() / 1000) - at);
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  return `${Math.floor(s / 86400)}d`;
}

/** The personal todo list — a quick-capture checklist on the rail. Add
 * items the moment they come to mind (the input is focused on open, or
 * `lazed todo add` from any terminal); check them off, clear the done. */
export function TodoView({
  items,
  error,
  onChanged,
  onFlash,
  onError,
}: {
  items: TodoItem[];
  /** e.g. "unknown method" when the running daemon predates todo.v1 */
  error?: string | null;
  onChanged: () => void;
  onFlash: (msg: string) => void;
  onError: (msg: string) => void;
}) {
  const [draft, setDraft] = useState("");
  const [showDone, setShowDone] = useState(false);
  const [editing, setEditing] = useState<{ id: string; text: string } | null>(
    null,
  );
  const addRef = useRef<HTMLInputElement>(null);

  const open = items.filter((i) => !i.done);
  const done = items.filter((i) => i.done);

  // capture is the point — the input is ready the moment the view opens
  useEffect(() => {
    addRef.current?.focus();
  }, []);

  // the inline rename input grabs focus when it mounts
  const editRef = useCallback((el: HTMLInputElement | null) => {
    el?.focus();
    el?.select();
  }, []);

  const run = (p: Promise<unknown>, ok?: string) =>
    p
      .then(() => {
        if (ok) onFlash(ok);
        onChanged();
      })
      .catch((e) => onError(String(e)));

  const capture = () => {
    const title = draft.trim();
    if (!title) return;
    setDraft("");
    run(todo.add(title));
  };

  const commitEdit = () => {
    if (!editing) return;
    const title = editing.text.trim();
    const prev = items.find((i) => i.id === editing.id)?.title;
    setEditing(null);
    if (title && title !== prev) {
      run(todo.update(editing.id, { title }));
    }
  };

  const check = (item: TodoItem) => (
    <button
      type="button"
      className="td-check"
      title={item.done ? "reopen" : "mark done"}
      onClick={() => run(todo.update(item.id, { done: !item.done }))}
    >
      <HugeiconsIcon
        icon={item.done ? CheckmarkSquare02Icon : SquareIcon}
        size={15}
        strokeWidth={1.5}
      />
    </button>
  );

  const row = (item: TodoItem) => (
    <div key={item.id} className={`ibx-row td-row ${item.done ? "done" : ""}`}>
      {check(item)}
      <div className="ibx-main">
        {editing?.id === item.id ? (
          <input
            ref={editRef}
            className="ibx-add-input td-edit"
            value={editing.text}
            onChange={(e) => setEditing({ id: item.id, text: e.target.value })}
            onBlur={commitEdit}
            onKeyDown={(e) => {
              if (e.key === "Enter") commitEdit();
              if (e.key === "Escape") setEditing(null);
            }}
          />
        ) : (
          <div
            className="ibx-title"
            title="double-click to rename"
            onDoubleClick={() => setEditing({ id: item.id, text: item.title })}
          >
            {item.title}
          </div>
        )}
        <div className="ibx-meta">
          <span>
            {relTime(item.done ? (item.done_at ?? item.at) : item.at)}
          </span>
        </div>
      </div>
      <div className="ibx-actions">
        <button
          type="button"
          className="ibx-btn"
          title="delete"
          onClick={() => run(todo.remove(item.id))}
        >
          <HugeiconsIcon icon={Cancel01Icon} size={12} strokeWidth={1.5} />
        </button>
      </div>
    </div>
  );

  return (
    <div className="ibx-screen">
      <div className="ibx-col">
        <div className="side-head">
          <span className="side-head-label">
            Todo{open.length > 0 ? ` · ${open.length}` : ""}
          </span>
          <button
            type="button"
            className="side-head-btn"
            title="add a todo"
            onClick={() => addRef.current?.focus()}
          >
            <HugeiconsIcon icon={PlusSignIcon} size={17} strokeWidth={1.5} />
          </button>
        </div>
        <div className="ibx-add">
          <input
            ref={addRef}
            className="ibx-add-input"
            placeholder="add a todo… (⏎ adds)"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") capture();
            }}
          />
        </div>
        <div className="side-scroll">
          {error ? (
            <div className="ibx-empty">
              todo unavailable: {error}
              <br />
              <span className="ibx-empty-sub">
                an older daemon may be running — restart it after rebuilding
              </span>
            </div>
          ) : (
            items.length === 0 && (
              <div className="ibx-empty">
                nothing here — add whatever comes to mind
                <br />
                <span className="ibx-empty-sub">lazed todo add</span>
              </div>
            )
          )}
          {open.map(row)}
          {done.length > 0 && (
            <div className="td-done-bar">
              <button
                type="button"
                className="ibx-snoozed-toggle"
                onClick={() => setShowDone((v) => !v)}
              >
                {showDone ? "▾" : "▸"} done ({done.length})
              </button>
              <button
                type="button"
                className="td-clear"
                title="delete all done items"
                onClick={() => run(todo.clear(), `cleared ${done.length}`)}
              >
                clear
              </button>
            </div>
          )}
          {showDone && done.map(row)}
        </div>
      </div>
    </div>
  );
}
