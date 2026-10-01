import { invoke } from "@tauri-apps/api/core";

export type InboxStatus = "open" | "snoozed" | "done";

/** One captured item in the daemon-owned GTD inbox. */
export interface InboxItem {
  id: string;
  /** where it came from — "manual", an automation name, "jira", "slack"… */
  source: string;
  /** integration that produced it — lowercase slug like "jira", "slack",
   * "gmail". Set by the producer (automation preset, JSON `provider` field,
   * `--provider`); the inbox groups by it, falling back to `source`. */
  provider?: string;
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
    opts?: {
      body?: string;
      url?: string;
      source?: string;
      provider?: string;
      key?: string;
    },
  ) => invoke<InboxItem>("inbox_add", { params: { ...opts, title } }),
  update: (
    id: string,
    patch: { status?: InboxStatus; snooze_until?: number },
  ) => invoke<InboxItem>("inbox_update", { params: { id, ...patch } }),
  remove: (id: string) => invoke("inbox_remove", { id }),
};

/** Open a source link in the desktop browser. */
export const openUrl = (url: string) => invoke("open_url", { url });

/** Provider slugs the inbox recognizes — used to label/icon sections and to
 * infer the provider of legacy items whose `source` stores an automation
 * name like "Jira — issues assigned to me (REST)". */
export const INBOX_PROVIDERS = [
  "jira",
  "slack",
  "gmail",
  "github",
  "gitlab",
  "notion",
  "linear",
  "asana",
  "trello",
  "figma",
  "discord",
  "google",
  "calendar",
  "todoist",
  "outlook",
  "confluence",
  "teams",
  "zoom",
  "monday",
  "clickup",
] as const;

/** Group key for an inbox row: the explicit provider slug when set, else a
 * provider inferred from the source's leading word, else "manual" for quick
 * captures, else the source itself — a custom automation groups under its
 * own name. */
export function providerOf(
  item: Pick<InboxItem, "provider" | "source">,
): string {
  const p = item.provider?.trim().toLowerCase();
  if (p) return p;
  const s = (item.source ?? "").trim().toLowerCase();
  if (!s || s === "manual") return "manual";
  for (const id of INBOX_PROVIDERS) {
    if (s === id || (s.startsWith(id) && /[^a-z0-9]/.test(s[id.length] ?? "")))
      return id;
  }
  return item.source.trim();
}

/** Jira issue key for clustering rows inside the "jira" provider group —
 * the dedupe key's head before '#' (mentions emit "CS-1#commentId"), else
 * the first PROJ-123 token in the title or url. Null when nothing matches. */
export function jiraIssueOf(
  item: Pick<InboxItem, "key" | "title" | "url">,
): string | null {
  const head = item.key?.split("#")[0]?.trim() ?? "";
  const m =
    /^[A-Za-z][A-Za-z0-9]*-\d+$/.exec(head) ??
    /[A-Za-z][A-Za-z0-9]*-\d+/.exec(`${item.title} ${item.url ?? ""}`);
  return m ? m[0].toUpperCase() : null;
}
