import { invoke } from "@tauri-apps/api/core";

export type InboxStatus = "open" | "snoozed" | "done";

/** One captured item in the daemon-owned GTD inbox. */
export interface InboxItem {
  id: string;
  /** where it came from — "manual", an automation name, "jira", "slack"… */
  source: string;
  /** dedupe key within the source (jira issue key, slack ts…) */
  key?: string;
  title: string;
  body?: string;
  url?: string;
  /** epoch seconds — when the item landed */
  at: number;
  status: InboxStatus;
  snooze_until?: number;
}

export const inbox = {
  list: (all = false) =>
    invoke<{ items: InboxItem[] }>("inbox_list", { all }).then((r) => r.items),
  add: (
    title: string,
    opts?: { body?: string; url?: string; source?: string; key?: string },
  ) => invoke<InboxItem>("inbox_add", { params: { ...opts, title } }),
  update: (
    id: string,
    patch: { status?: InboxStatus; snooze_until?: number },
  ) => invoke<InboxItem>("inbox_update", { params: { id, ...patch } }),
  remove: (id: string) => invoke("inbox_remove", { id }),
};

/** Open a source link in the desktop browser. */
export const openUrl = (url: string) => invoke("open_url", { url });
