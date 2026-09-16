import { invoke } from "@tauri-apps/api/core";

/** What an automation does with each NEW item its poller finds.
 * - collect: just list it in the panel (fire manually per item)
 * - notify: macOS notification
 * - command: run `command` template — {id}/{text} substituted shell-quoted
 * - agent: new terminal in `project_id` → agent `agent_kind` → `prompt` template */
export type ActionKind = "collect" | "notify" | "command" | "agent";

export interface AutomationAction {
  kind: ActionKind;
  command?: string;
  agent_kind?: string;
  project_id?: string;
  prompt?: string;
}

export interface AutomationItem {
  id: string;
  text: string;
  /** epoch seconds */
  at: number;
  fired: boolean;
}

export interface Automation {
  id: string;
  name: string;
  enabled: boolean;
  interval_secs: number;
  command: string;
  action: AutomationAction;
  /** number of ids already recorded (first poll seeds without firing) */
  seen_count: number;
  seeded: boolean;
  last_run_at?: number;
  last_ok_at?: number;
  last_error?: string;
  last_items: AutomationItem[];
  fire_count: number;
}

/** Editable subset sent to automation_save — id empty = create. */
export interface AutomationInput {
  id?: string;
  name: string;
  enabled: boolean;
  interval_secs: number;
  command: string;
  action: AutomationAction;
}

export interface TestResult {
  ok: boolean;
  items?: { id: string; text: string }[];
  error?: string;
}

export const automations = {
  list: () =>
    invoke<{ automations: Automation[] }>("automation_list").then(
      (r) => r.automations,
    ),
  save: (input: AutomationInput) =>
    invoke<Automation>("automation_save", { input }),
  delete: (id: string) => invoke("automation_delete", { id }),
  setEnabled: (id: string, enabled: boolean) =>
    invoke("automation_set_enabled", { id, enabled }),
  runNow: (id: string) => invoke("automation_run_now", { id }),
  resetSeen: (id: string) => invoke("automation_reset_seen", { id }),
  fire: (id: string, itemId: string) =>
    invoke("automation_fire", { id, itemId }),
  test: (command: string) => invoke<TestResult>("automation_test", { command }),
};

/** Poller presets — the command is any shell producing one item per line
 * (JSON lines or `id…text`); placeholders are meant to be edited. */
export const AUTOMATION_PRESETS: {
  id: string;
  name: string;
  command: string;
  hint: string;
}[] = [
  {
    id: "jira-cli",
    name: "Jira — issues mentioning me (jira CLI)",
    command: `jira issue list --plain -q 'text ~ "YOUR_JIRA_USERNAME" AND resolution = EMPTY ORDER BY updated DESC' | grep -E '^[A-Z][A-Z0-9]+-[0-9]+'`,
    hint: "requires ankitpokhrel/jira-cli (`jira init` first); replace YOUR_JIRA_USERNAME",
  },
  {
    id: "jira-rest",
    name: "Jira — mentions via REST API",
    command: `curl -s -u "$JIRA_EMAIL:$JIRA_API_TOKEN" "$JIRA_BASE/rest/api/3/search/jql?jql=text%20~%20%22$JIRA_USER%22%20AND%20resolution%20=%20EMPTY&fields=summary&maxResults=50" | python3 -c 'import json,sys\\nfor i in json.load(sys.stdin).get("issues",[]): print(f"{i[\\"key\\"]}\\t{i[\\"fields\\"][\\"summary\\"]}")'`,
    hint: "set JIRA_BASE / JIRA_EMAIL / JIRA_API_TOKEN / JIRA_USER env vars in the command",
  },
  {
    id: "gh-review",
    name: "GitHub — PRs requesting my review",
    command: `gh search prs --review-requested=@me --state=open --json number,title,repository --jq '.[] | "\\(.repository.nameWithOwner)#\\(.number)\\t\\(.title)"'`,
    hint: "requires gh CLI logged in",
  },
  {
    id: "custom",
    name: "Custom command",
    command: "",
    hint: "any shell command — one item per line, first token = item id",
  },
];
