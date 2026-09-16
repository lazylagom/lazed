---
name: lazed
description: Orchestrate agents inside lazed terminals via the `lazed` CLI — create worktrees, spawn agents (claude/codex/antigravity/devin), prompt them, wait on status, verify output.
---

# lazed orchestration

`lazed` is a headless terminal-workspace daemon. It owns real PTYs — the GUI
can die and every agent keeps running. You are (usually) the **orchestrator**:
a terminal in the project's root checkout. You fan work out to worker
terminals (typically git worktrees) and supervise them.

## Model

```
session (one daemon)
├─ group    gN   = optional named collection of projects (sidebar section)
└─ project  pN   = one git repo
   ├─ terminal t1  (root checkout — you)
   ├─ terminal t2  (worktree)
   └─ terminal t3  (worktree)
```

- IDs are `gN` / `pN` / `tN`. `LAZED_TERM` env var inside a terminal = its own id.
- A project sits in at most one group; ungrouped projects render flat in the
  sidebar. Removing a group never closes its projects.
- Terminal kinds: `worktree`, `plain`. A project's first terminal opens in
  the repo's root checkout — that's where you coordinate.
- Socket: `~/.local/state/lazed/lazed.sock` (override: `LAZED_STATE_DIR`).
- Everything below is `lazed api <method> '<json-params>'` → JSON result.

## Commands

```sh
lazed status                                    # daemon up?
lazed api session.snapshot                      # full model: groups + projects + terminals

# projects
lazed api project.create  '{"cwd":"/repo"}'     # → project + root terminal
lazed api project.create  '{"cwd":"/repo","group_id":"g1"}'  # straight into a group
lazed api project.list
lazed api project.close   '{"project_id":"p1"}'

# groups — named project collections
lazed api group.create  '{"label":"Work"}'                    # → {group_id:"g1",...}
lazed api group.list
lazed api group.assign  '{"project_id":"p2","group_id":"g1"}' # move into group
lazed api group.assign  '{"project_id":"p2","group_id":""}'   # ungroup
lazed api group.rename  '{"group_id":"g1","label":"New"}'
lazed api group.remove  '{"group_id":"g1"}'                   # projects stay, ungrouped

# terminals
lazed api terminal.create '{"project_id":"p1","command":"htop"}'
lazed api terminal.input  '{"term_id":"t2","text":"ls\n"}'   # \n = enter
lazed api terminal.read   '{"term_id":"t2","lines":50}'      # screen text
lazed api terminal.close  '{"term_id":"t2"}'

# worktrees — creates git worktree AND its terminal under the project
lazed api worktree.create '{"project_id":"p1","branch":"feat-x"}'
                          # → {checkout_path, branch, terminal:{term_id}}
lazed api worktree.remove '{"term_id":"t3"}'                 # kills term + git rm

# agents — kinds: claude, codex, antigravity, devin, pi, ...
lazed api agent.start  '{"term_id":"t3","kind":"claude"}'    # runs `claude` in t3
lazed api agent.start  '{"term_id":"t3","spec":"gpt-5.6-luna"}'  # named spec → kind+args
lazed api agent.specs                                          # list registered specs
# launch with flags / one-shot prompt — pass argv, the daemon shell-quotes it.
# NEVER hand-build a quoted command line inside terminal.input text.
lazed api agent.start  '{"term_id":"t3","kind":"pi","args":["--provider","openai-codex","--model","gpt-5.6-luna","--no-session","-p","analyze this repo, output in Korean"]}'
lazed api agent.prompt '{"term_id":"t3","text":"fix the tests"}'
lazed api agent.get    '{"term_id":"t3"}'                    # kind + status + fg (foreground proc)
lazed api agent.wait   '{"term_id":"t3","until":["idle","blocked"],"timeout_ms":600000}'

# attach stream (raw PTY-level channel — for scripts, not usually needed)
echo '{"type":"input","text":"ls\n"}' | lazed term attach t3 --cols 80 --rows 24
```

## Agent statuses

`working` (spinner/esc-to-interrupt) · `blocked` (approval prompt on
screen) · `done` (finished its turn — sticky until the next transition) ·
`idle` (alive, waiting for input) · `unknown` (no agent detected).

`done` = `working` followed by quiet screen; a wait for `idle` also
matches `done`. `unknown` is NOT "finished" — always `terminal.read`
before assuming.

After `agent.start`, verify the launch actually took: `agent.get` should
show `agent` = the kind AND `fg.comm` = the agent binary. If `agent` is
null / `fg` is still `zsh`/`bash`, the command line never ran — a common
cause is a quoting slip leaving the shell stuck on a continuation prompt
(screen shows a lone `∙` or `>` line, NOT a spinner). `terminal.read`,
fix the line (Esc/Ctrl-C first), and retry with `args` instead of raw text.

## Invocation options

The first args after `/skill:lazed` may carry options:

```text
/skill:lazed <task>                          — your judgment: do it yourself or delegate
/skill:lazed --task <task>                   — same, with the task marked explicitly
/skill:lazed --agent --task <task>           — always delegate, the registry default spec
/skill:lazed --agent <spec> --task <task>    — always delegate to that spec
```

Parsing rules:

- `--task` is terminal: everything after it is the task verbatim. Put all
  options before `--task`.
- `--agent` optionally takes one spec token — consumed only when it names
  a registered spec or a known agent kind (`agent.specs` lists them).
- `/skill:lazed --agent <task>` without `--task` still works: if the
  token after `--agent` isn't a spec, `--agent` is bare and the rest is
  the task. Prefer `--task` whenever the task could start with a
  spec-looking word — it removes the guesswork entirely.

`--agent` forces worktree delegation — even a single small task gets its
own worktree worker. Specs are `{kind, args}` pairs in
`~/.config/lazed/agents.json` — edit it to add models/wrappers; `default`
picks the spec a bare `--agent` uses.

## Fan-out playbook

With no `--agent`, use judgment: questions, status checks, and trivial
lookups you answer yourself; substantial repo work goes to a worktree
worker (a single task still gets one worktree). With `--agent`,
delegation is mandatory — never silently do the work in the main
checkout instead. Worktree isolation keeps the main checkout clean and
leaves your own terminal free to supervise.

For a task too big for one terminal:

1. Decompose into independent subtasks (one per worktree).
2. For each: `worktree.create` → `agent.start` (spec or kind) →
   `agent.prompt` with a self-contained task description (include the
   branch name + checkout path).
3. Per terminal: prompt → wait for `working` first (poll `agent.get`,
   ~30s budget — otherwise `agent.wait` may return on the pre-prompt idle)
   → `agent.wait until=[idle,blocked]` → `terminal.read` to verify.
4. `blocked` = approval prompt. `terminal.read` it, then `terminal.input`
   the exact keys it asks for (e.g. `1\n` for "1. Yes"), never free text.
5. Merge back in YOUR terminal (the main checkout): `git merge
   <branch>` or ask the user. Then `worktree.remove` the worker.
6. Report per-worktree summary: branch, status, what changed.

## Safety

- Never close terminals/projects you didn't create; never `lazed stop`.
- Never merge or delete a worktree before `terminal.read` shows the agent
  is done (not just quiet — check `agent.get` status is `idle`/`blocked`).
- One task per worktree. If a task is ambiguous, ask the user instead of
  guessing — wrong-branch work is expensive to untangle.
