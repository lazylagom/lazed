import { invoke } from "@tauri-apps/api/core";

/** What an automation does with each NEW item its poller finds.
 * - collect: just list it in the panel (fire manually per item)
 * - notify: macOS notification
 * - command: run `command` template — {id}/{text} substituted shell-quoted
 * - agent: new terminal in `project_id` → agent `agent_kind` → `prompt` template
 * - inbox: drop into the daemon inbox for triage */
export type ActionKind = "collect" | "notify" | "command" | "agent" | "inbox";

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
  /** source link — from url/link/permalink in JSON lines, or the first
   * http(s) token in plain lines */
  url?: string;
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
  /** catalog preset id this was switched on from (see AUTOMATION_PRESETS) */
  preset?: string;
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
  preset?: string;
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
 * (JSON lines or `id…text`); placeholders are meant to be edited.
 * `requires` declares what the spawn env must provide — the editor checks
 * it and links gaps to Settings → Integrations or an install hint.
 *
 * Every preset except `custom` is also a ready-made automation on the
 * Automations screen: switching it on creates the automation from these
 * defaults (`defaults`), switching it off pauses it. */
export interface AutomationPreset {
  id: string;
  /** short chip label for the picker */
  tag: string;
  name: string;
  /** one-line description for the catalog row */
  sub?: string;
  command: string;
  hint: string;
  requires?: { bins?: string[]; env?: string[] };
  /** how the automation is created when switched on from the catalog */
  defaults?: { action: AutomationAction; interval_secs: number };
}

export const PRESET_DEFAULTS: NonNullable<AutomationPreset["defaults"]> = {
  action: { kind: "inbox" },
  interval_secs: 300,
};

/** The AutomationInput that switching a catalog preset on creates. */
export function presetInput(p: AutomationPreset): AutomationInput {
  const d = p.defaults ?? PRESET_DEFAULTS;
  return {
    name: p.name.split(" — ")[0],
    enabled: true,
    interval_secs: d.interval_secs,
    command: p.command,
    action: d.action,
    preset: p.id,
  };
}

/** The stored automation a catalog preset maps to — by recorded preset id,
 * or (for automations created before ids were recorded) by an unedited
 * command. */
export function automationForPreset(
  p: AutomationPreset,
  autos: Automation[],
): Automation | undefined {
  return (
    autos.find((a) => a.preset === p.id) ??
    autos.find((a) => !a.preset && p.command && a.command === p.command)
  );
}

export const AUTOMATION_PRESETS: AutomationPreset[] = [
  {
    id: "jira-rest",
    tag: "jira",
    name: "Jira — issues assigned to me (REST)",
    sub: "Open issues assigned to you — one item per issue, linked to the issue.",
    command: `curl -s -u "$JIRA_EMAIL:$JIRA_API_TOKEN" "$JIRA_BASE/rest/api/3/search/jql?jql=assignee%20%3D%20currentUser()%20AND%20resolution%20%3D%20EMPTY%20ORDER%20BY%20updated%20DESC&fields=summary&maxResults=50" | python3 -c 'import json,sys,os; base=os.environ.get("JIRA_BASE","").rstrip("/"); [print(json.dumps({"id": i["key"], "title": i["fields"]["summary"], "url": base + "/browse/" + i["key"]})) for i in json.load(sys.stdin).get("issues", [])]'`,
    hint: "connect Jira in Settings → Integrations (or export JIRA_BASE / JIRA_EMAIL / JIRA_API_TOKEN yourself) — JSON lines carry the issue URL",
    requires: { env: ["JIRA_BASE", "JIRA_EMAIL", "JIRA_API_TOKEN"] },
  },
  {
    id: "jira-mention",
    tag: "jira @me",
    name: "Jira mention @me",
    sub: "Comments (or descriptions) that @-mention you — one item per mention, linked to the comment.",
    command: `python3 - <<'LAZED_JIRA_MENTIONS'
import base64, json, os, sys, urllib.error, urllib.parse, urllib.request

base = os.environ.get("JIRA_BASE", "").rstrip("/")
user = os.environ.get("JIRA_EMAIL", "")
token = os.environ.get("JIRA_API_TOKEN", "")
if not base or not token:
    sys.exit("JIRA_BASE / JIRA_API_TOKEN not set — connect Jira in Settings")
auth = ("Basic " + base64.b64encode((user + ":" + token).encode()).decode()) if user else ("Bearer " + token)

def get(path):
    req = urllib.request.Request(base + path, headers={"Authorization": auth, "Accept": "application/json"})
    with urllib.request.urlopen(req, timeout=20) as r:
        return json.load(r)

api, me = "3", None
for v in ("3", "2"):
    try:
        api, me = v, get("/rest/api/" + v + "/myself")
        break
    except urllib.error.HTTPError as e:
        if e.code in (401, 403):
            sys.exit("jira auth failed (%s) — recheck the token in Settings" % e.code)
    except Exception:
        pass
if me is None:
    sys.exit("cannot reach " + base)
account = me.get("accountId") or me.get("name") or ""
marker = "[~accountid:%s]" % account if me.get("accountId") else "[~%s]" % account
handle = "@" + (me.get("displayName") or "me")

def mentions_me(node):
    if isinstance(node, dict):
        if node.get("type") == "mention" and (node.get("attrs") or {}).get("id") == account:
            return True
        return mentions_me(node.get("content") or [])
    if isinstance(node, list):
        return any(mentions_me(c) for c in node)
    if isinstance(node, str):
        return marker in node
    return False

def flatten(node, out):
    if isinstance(node, dict):
        if node.get("type") == "text":
            out.append(node.get("text") or "")
        elif node.get("type") == "mention":
            out.append((node.get("attrs") or {}).get("text") or "@?")
        flatten(node.get("content") or [], out)
    elif isinstance(node, list):
        for c in node:
            flatten(c, out)
    return out

def excerpt(body):
    raw = body if isinstance(body, str) else " ".join(flatten(body, []))
    return " ".join(raw.replace(marker, handle).split())[:180] or "(no text)"

jql = '(comment ~ "{m}" OR description ~ "{m}") AND updated >= -14d ORDER BY updated DESC'.format(m=marker)
query = urllib.parse.urlencode({"jql": jql, "fields": "summary,comment,description", "maxResults": 25})
search = "/rest/api/3/search/jql?" if api == "3" else "/rest/api/2/search?"
for issue in get(search + query).get("issues") or []:
    key = issue.get("key") or ""
    fields = issue.get("fields") or {}
    summary = fields.get("summary") or ""
    link = "%s/browse/%s" % (base, key)
    comments = ((fields.get("comment") or {}).get("comments")) or []
    if not comments:
        try:
            comments = get("/rest/api/%s/issue/%s/comment?maxResults=50" % (api, key)).get("comments") or []
        except Exception:
            comments = []
    for c in comments:
        body = c.get("body")
        if not mentions_me(body):
            continue
        who = (c.get("author") or {}).get("displayName") or "someone"
        cid = c.get("id") or ""
        print(json.dumps({
            "id": "%s#%s" % (key, cid),
            "title": "%s · %s: %s" % (summary, who, excerpt(body)),
            "url": "%s?focusedCommentId=%s" % (link, cid),
        }))
    if mentions_me(fields.get("description")):
        print(json.dumps({"id": key + "#description", "title": "%s · mentioned in the description" % summary, "url": link}))
LAZED_JIRA_MENTIONS`,
    hint: "connect Jira in Settings → Integrations — one item per comment (or description) that @-mentions you in the last 14 days; the link opens that comment",
    requires: { env: ["JIRA_BASE", "JIRA_API_TOKEN"] },
  },
  {
    id: "slack-channel",
    tag: "slack",
    name: "Slack — channel messages (REST)",
    sub: "New messages in one Slack channel — one item per message.",
    command: `curl -s -H "Authorization: Bearer $SLACK_TOKEN" "https://slack.com/api/conversations.history?channel=$SLACK_CHANNEL&limit=30" | python3 -c 'import json,sys,os; d=json.load(sys.stdin); ch=os.environ.get("SLACK_CHANNEL",""); ws=os.environ.get("SLACK_WORKSPACE",""); [print(json.dumps({"id": m.get("ts",""), "title": (m.get("text") or "(no text)")[:200], "url": ("https://" + ws + ".slack.com/archives/" + ch + "/p" + m.get("ts","").replace(".","")) if ws else ""})) for m in d.get("messages", [])]'`,
    hint: "set SLACK_TOKEN (xoxp/xoxb with channels:history) / SLACK_CHANNEL / SLACK_WORKSPACE — pair with the inbox action",
    requires: { env: ["SLACK_TOKEN", "SLACK_CHANNEL", "SLACK_WORKSPACE"] },
  },
  {
    id: "gh-review",
    tag: "github",
    name: "GitHub — PRs requesting my review",
    sub: "Open pull requests where your review is requested.",
    command: `gh search prs --review-requested=@me --state=open --json number,title,repository --jq '.[] | "\\(.repository.nameWithOwner)#\\(.number)\\t\\(.title)"'`,
    hint: "requires gh CLI logged in (`gh auth login`)",
    requires: { bins: ["gh"] },
  },
  {
    id: "custom",
    tag: "custom",
    name: "Custom command",
    command: "",
    hint: "any shell command — one item per line, first token = item id",
  },
];
