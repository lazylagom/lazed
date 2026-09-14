---
name: staylazy
description: Orchestrate agents running inside staylazy panes via the herdr CLI — read output, send prompts, spawn panes, wait on states.
---

# staylazy orchestration

staylazy is a GUI client on top of the herdr server. Every pane you see is a
real herdr pane (`w1:pN`), every workspace is a herdr workspace (`wN`), and all
herdr CLI/API capabilities apply unchanged. If a `herdr` skill is loaded, its
rules apply here too.

## Key facts

- Pane IDs look like `w1:p3`, tabs `w1:t2`, workspaces `w4`. They are stable
  across server restarts.
- `herdr pane list`, `herdr api snapshot` give the full live model as JSON.
- `herdr agent start <name> --kind <kind> --pane <pane>` starts an interactive
  agent in an existing shell pane. `herdr agent prompt <pane> <text>` submits a
  prompt; `--wait --until done|blocked` blocks for completion.
- `herdr agent get <pane>` / `herdr agent explain <pane>` report detection
  state. Statuses: `working`, `blocked`, `done`, `idle`, `unknown`.
  `unknown` is NOT "finished" — read the pane before assuming.
- `herdr pane read <pane>` dumps the visible screen; `herdr pane run <pane>
  <cmd>` sends a shell command; `herdr pane send-text/send-keys` for raw input.
- `herdr pane split <pane> --direction right|down` and `herdr tab create
  --workspace <ws>` appear in the staylazy GUI immediately (live event feed).

## Patterns

- Fan-out: create N workspaces (`herdr worktree create --cwd <repo> --branch
  <b>` opens one per worktree), start the same agent kind in each pane, then
  `agent prompt` each with the same task. Compare results via `pane read`.
- Pipelines: `herdr agent wait <pane> --until done` then send follow-up input.
- Blocked agents: before sending input to a `blocked` pane, `pane read` it —
  it is usually an approval prompt needing a specific key, not free text.

## Safety

- Do not close panes/workspaces you did not create.
- Do not run `herdr server stop` — the GUI owns the server lifecycle and will
  restart it; stopping mid-task kills in-flight streams.
