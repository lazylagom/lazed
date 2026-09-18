---
name: lazed
description: Delegate coding work to another terminal — start a named agent in a sibling pane or a separate Git worktree/branch, send follow-ups, read its output, and check progress. Use when the user asks to run/spawn/delegate work to another agent (claude, codex, pi, devin), to work in a worktree or separate branch, to review or fix something "in parallel" or "elsewhere", or mentions lazed, panes, or named agents. Not for plain `git worktree` questions answered by reading the repo.
---

# lazed

Treat **panes and worktrees as locations**, and **named agents as ongoing
conversation partners**. The user requests work in natural language; choose
the commands below without making the user manage task IDs or JSON.

If `lazed` is not on PATH (`command not found`), the CLI/skill links were never
installed — ask the user to run `lazed install` once (the app's first-run
banner offers the same).

## Discover and choose the location

Read `lazed status`, then `lazed agent`, `lazed pane`, or `lazed worktree`
for the relevant syntax. Bare groups and `--help` are read-only discovery.
The running daemon must advertise `agent.names.v1` and `pane.context.v1`.
A rebuilt CLI does not upgrade a live daemon. Do not stop/restart it on a
missing-capability error; existing processes would be lost. Restart only when
the user explicitly requests it, after checking active work and backing up
state. If the controlling agent itself lives in lazed, restarting also ends
that conversation process; use an external handoff rather than stopping it
mid-task.

Use the user's chosen agent kind or registered spec (`lazed agent specs`).
When the request names no agent, **use the same kind as the caller** — the
orchestrator's own agent — so delegated work runs in a familiar environment.
Read it from the caller's own pane (`lazed agent get "$LAZED_TERM"`), not from a guess.
Fall back to the configured default only when the caller's kind is unknown or
unsupported, and ask when neither is available.

- By default create a sibling pane in the **caller's current tab and cwd**,
  without changing focus. A dirty checkout is valid here: the agent sees the
  same files, including uncommitted changes.
- Create a new worktree only when the user asks for isolation, a separate
  branch/worktree, or independent work explicitly placed there.
- Additional instructions, progress checks, and output reads target the
  existing named agent. They do not create another pane or worktree.
- `agent start` requires an available shell pane. It never creates layout.
  Don't start an agent in the caller's own occupied pane.
- Use `LAZED_TERM` through `--current`, an explicit pane ID, or a live name.
  Never infer the target from another client's GUI focus. If outside a lazed
  pane, inspect `pane list` and ask for a target when the request doesn't
  identify one; don't silently substitute a worktree.

Examples of natural requests:

- “lazed로 codex를 띄워서 현재 변경 리뷰해줘” → sibling pane, named reviewer.
- “별도 worktree에서 pi로 로그인 버그 고쳐줘” → worktree, named login-fix.
- “아까 reviewer에게 회귀 테스트도 확인하라고 해줘” → same agent, follow-up.
- “진행 상황 보여줘” → inspect the existing agent; no new process.

The existing invocation `/skill:lazed --agent pi --task <request>` remains
usable: `--agent` chooses/forces delegation, **not worktree creation**.
`--task` ends invocation-option parsing and preserves the remaining request.
Bare `--agent` uses the registry default. These are skill options, not CLI flags.

## Agent in a sibling pane

```sh
lazed pane current --current
lazed pane split --current --cwd "$PWD" --no-focus
```

Read the new `.pane.term_id`; don't guess IDs. Use a meaningful unique name:

```sh
lazed agent start reviewer --kind codex --pane <returned-term-id>
lazed agent prompt reviewer "현재 변경을 리뷰하고 실제 문제만 알려줘." --wait --timeout 120000
lazed agent read reviewer --source recent-unwrapped --lines 120
lazed agent prompt reviewer "빈 입력 처리도 확인해줘." --wait --timeout 120000
```

Use `--spec NAME` for a configured launcher/model. Pass native agent arguments
only after `--` on start. Start waits for the expected process and interactive
readiness. If startup is blocked, the name and pane are retained for inspection;
after the user resolves the dialog, wait/get and prompt that same agent instead
of launching it again.

Names match `[a-z][a-z0-9_-]{0,31}`, exclude pane-ID shapes such as `t2`, and
are unique among live named agents. Names expire when the occupant exits or
is replaced; daemon restart does not restore live agents or their names.
If a name is occupied, inspect it instead of attaching a new meaning to it.

For long/multiline prompts use `--prompt-file PATH` or `--stdin`; create files
with the host's safe editing mechanism, never interpolate user text into shell
commands. Prompt input is bracketed paste followed by Enter. When sent from
idle, `--wait` observes post-submission activity before a settled state, so old
idle cannot finish it. With `agent.busy_prompt.v1`, a working agent also accepts
follow-ups: its native CLI decides whether to queue or steer the request.
`started_while_working` and `wait_scope: agent_lifecycle` describe that receipt.
This is not a per-request completion guarantee: the already-running turn can
satisfy the wait. Read the response and verify the follow-up was handled.

## Agent in a worktree

```sh
lazed worktree create --repo "$PWD" --branch login-fix
```

This creates a workspace, tab, and shell pane. Use the returned
`.terminal.term_id` to start the agent; the workflow is otherwise identical:

```sh
lazed agent start login-fix --kind pi --pane <returned-term-id>
lazed agent prompt login-fix "로그인 버그를 수정하고 관련 테스트를 실행해줘." --wait --timeout 120000
lazed agent get login-fix
lazed agent read login-fix --lines 120
```

Default base is the caller checkout's resolved HEAD. `--base REF` selects
another base. Uncommitted changes are **not copied**: a dirty source is rejected
unless `--allow-dirty` is explicitly chosen with that exclusion understood.
Never silently stash/copy changes. Branch/path collisions fail; inspect
`lazed worktree list --repo "$PWD"` before creating another checkout.

## Supervise the same agent

```sh
lazed agent list
lazed agent get reviewer
lazed agent wait reviewer --timeout 120000
lazed agent read reviewer --lines 120
```

`idle`/`done` means ready for input, not verified implementation success.
Check output, diff and tests separately. `working` accepts follow-up text
without another pane/worktree; it does not authorize interrupting the agent.
`blocked` needs inspection and possibly user approval; task text is rejected.
`unknown` is not completion and does not accept ordinary prompts.

If wait times out or submission is uncertain, inspect get/read; don't repeat
the prompt. A request may have been delivered even when the client failed.
Never paste task text into trust/login/approval UI. When authorized to answer
or interrupt, use logical keys, validated before any input is written:

```sh
lazed agent send-keys reviewer esc
lazed agent send-keys reviewer ctrl+c
```

`read` supports visible/detection and recent/recent-unwrapped screen+scrollback.
With `agent.history.v1`, long recent reads of managed, idle Claude/Codex/Pi
agents can collect alternate-screen history through supported SGR mouse
scrolling, then restore the viewport. Working/blocked agents should be read
with `--source visible`. User-scrolled or unsupported views remain passive.
Inspect the returned `history` metadata: `method`, `viewport_restored`, and
`note` expose partial reads or cancellation. Never treat a partial viewport
as the complete answer. If recovery fails, ask the idle agent to write its
answer to a temporary Markdown file and read that file as a fallback.

## Boundaries and compatibility

- Managed readiness currently supports Claude, Codex, Devin and direct Pi
  launchers. Claude/Codex/Devin use process plus screen evidence; Pi adds
  launch-scoped hooks. Unsupported/uncertain UI does not get a guessed ready
  state.
- Current lazed layout is a row of sibling panes, not Herdr's directional split
  tree. Don't invent `--direction` support. Wait defaults to 30 seconds and is
  bounded to 600000 ms, unlike Herdr's optional indefinite wait.
- No automatic merge, commit, worktree deletion, or approval. Closing/removing
  resources or restarting the daemon requires explicit user scope.
- Existing `lazed task` records remain compatible. If the user identifies an
  old task, use `task status/read/tell/resume` for it instead of creating a
  replacement. Task records are optional compatibility, not the new default UX.
