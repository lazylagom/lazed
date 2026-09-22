---
name: lazed
description: Delegate coding work to another terminal — start a named agent in a sibling pane or a separate Git worktree/branch, send follow-ups, read its output, and check progress. Use when the user asks to run/spawn/delegate work to another agent (claude, codex, pi, devin), to work in a worktree or separate branch, to review or fix something "in parallel" or "elsewhere", or mentions lazed, herdr, panes, or named agents. Not for plain `git worktree` questions answered by reading the repo.
---

# lazed

lazed is a GUI + organization layer **on top of herdr**. Panes, tabs,
workspaces, agents, names and lifecycle states are herdr's — control them
with the `herdr` CLI exactly as the herdr skill describes (`herdr --skill`).
lazed adds only what herdr has no notion of:

- `lazed worktree` — git worktree lifecycle filed under a lazed project
  (hardened removal, branch cleanup, agent-occupancy check)
- `lazed task` — one worktree + one agent + one prompt as a durable record
  with a caller-chosen `request_id` (query after disconnect, never resend)
- `lazed inbox` — GTD capture queue shown in the lazed sidebar

Treat **panes and worktrees as locations**, and **named agents as ongoing
conversation partners**. The user requests work in natural language; choose
the commands below without making the user manage IDs or JSON.

If `herdr` is not on PATH the execution layer is missing — ask the user to
install herdr (the lazed app bundles one; `lazed install` links the CLI).
If `lazed` is missing, ask the user to run `lazed install` once.

## Discover and choose the location

herdr is the source of truth for layout and agents:

```sh
herdr pane current --current        # the caller's own pane (HERDR_PANE)
herdr pane list
herdr agent list
lazed api session.snapshot          # lazed's group > project filing on top
```

Use the user's chosen agent kind or a registered lazed spec
(`lazed api agent.specs`). When the request names no agent, **use the same
kind as the caller** — read it from the caller's own pane
(`herdr agent get "$HERDR_PANE"`), never guess. Fall back to the configured
default only when the caller's kind is unknown or unsupported.

- By default create a sibling pane in the **caller's current tab and cwd**,
  without changing focus. A dirty checkout is valid here: the agent sees the
  same files, including uncommitted changes.
- Create a new worktree only when the user asks for isolation, a separate
  branch/worktree, or independent work explicitly placed there.
- Follow-ups, progress checks and output reads target the existing named
  agent. They do not create another pane or worktree.
- `herdr agent start` requires an available shell pane and never creates
  layout. Don't start an agent in the caller's own occupied pane.
- Target with `--current`, an explicit pane ID (`w1:p3`) or a live agent
  name. Never infer the target from another client's GUI focus.

Examples of natural requests:

- “lazed로 codex를 띄워서 현재 변경 리뷰해줘” → sibling pane, named reviewer.
- “별도 worktree에서 pi로 로그인 버그 고쳐줘” → `lazed worktree create`, then a
  named agent in the returned pane.
- “아까 reviewer에게 회귀 테스트도 확인하라고 해줘” → same agent, follow-up.
- “진행 상황 보여줘” → inspect the existing agent; no new process.

## Agent in a sibling pane

```sh
herdr pane split --current --cwd "$PWD" --no-focus
herdr agent start reviewer --kind codex --pane <returned-pane-id>
herdr agent prompt reviewer "현재 변경을 리뷰하고 실제 문제만 알려줘." --wait --timeout 120000
herdr agent read reviewer --source recent-unwrapped --lines 120
herdr agent prompt reviewer "빈 입력 처리도 확인해줘." --wait --timeout 120000
```

Names match `[a-z][a-z0-9_-]{0,31}` and are unique among live agents. A name
follows the pane's current occupant and clears when that agent exits or is
released. If startup is blocked, herdr returns `agent_not_ready` but keeps the
name — inspect, let the user resolve the dialog, then wait/prompt that same
agent instead of starting it again.

## Agent in a worktree

```sh
lazed worktree create --repo "$PWD" --branch login-fix
```

This adds a git worktree, opens it as a herdr workspace with a shell pane,
and files it under the repo's lazed project. Use the returned `pane_id`:

```sh
herdr agent start login-fix --kind pi --pane <returned-pane-id>
herdr agent prompt login-fix "로그인 버그를 수정하고 관련 테스트를 실행해줘." --wait --timeout 120000
herdr agent get login-fix
herdr agent read login-fix --lines 120
```

Default base is the caller checkout's resolved HEAD; `--base REF` selects
another. Uncommitted changes are **not copied**: a dirty source is rejected
unless `--allow-dirty` is chosen with that exclusion understood. Branch/path
collisions fail; inspect `lazed worktree list --repo "$PWD"` first.

When the user wants the whole thing as one hands-off unit, `lazed task
start --branch login-fix --kind pi -- "<prompt>"` does worktree + agent +
first prompt and returns a `request_id`; `lazed task status|read|tell|resume
<id>` follow it.

## Supervise the same agent

```sh
herdr agent list
herdr agent get reviewer
herdr agent wait reviewer --timeout 120000
herdr agent read reviewer --lines 120
```

`idle`/`done` means ready for input, not verified implementation success —
check output, diff and tests separately. `working` accepts follow-ups; it
does not authorize interrupting. `blocked` needs inspection and possibly user
approval; task text is rejected. If a wait times out or submission is
uncertain, inspect get/read; don't repeat the prompt. Never paste task text
into trust/login/approval UI. When authorized to answer or interrupt:

```sh
herdr agent send-keys reviewer esc
```

## Removing a worktree

```sh
lazed worktree remove <workspace-id> [--force] [--kill-agents] [--keep-branch]
```

Refuses while a pane in the workspace still hosts an agent unless
`--kill-agents` is explicit. Dirty checkouts need `--force`. The branch is
deleted only when fully merged (or with `--force`); `--keep-branch` keeps it.

## Boundaries

- IDs (`w1`, `w1:t1`, `w1:p1`) and agent names are scoped to one herdr
  session. lazed attaches to the session named `lazed`; a bare `herdr` in a
  shell talks to the default session — pass `--session lazed` when working
  from outside a lazed pane.
- Never stop or restart the herdr server or the lazed daemon to fix an error;
  existing processes would be lost. Report and let the user decide.
- The lazed daemon keeps working without herdr (projects, inbox, task
  records), but nothing can run: `herdr_not_running` errors mean start herdr.
