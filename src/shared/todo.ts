import { invoke } from "@tauri-apps/api/core";

/** One item on the daemon-owned personal checklist. */
export interface TodoItem {
  id: string;
  title: string;
  /** epoch seconds — when the item was added */
  at: number;
  done: boolean;
  /** epoch seconds — when the item was checked off */
  done_at?: number;
}

export const todo = {
  list: () => invoke<{ items: TodoItem[] }>("todo_list").then((r) => r.items),
  add: (title: string) => invoke<TodoItem>("todo_add", { params: { title } }),
  update: (id: string, patch: { title?: string; done?: boolean }) =>
    invoke<TodoItem>("todo_update", { params: { id, ...patch } }),
  remove: (id: string) => invoke("todo_remove", { id }),
  /** drop every checked-off item */
  clear: () => invoke("todo_clear"),
};
