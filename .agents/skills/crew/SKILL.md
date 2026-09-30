---
name: crew
description: Run a task through a crew of coding agents on herdr — research, design, adversarial design review, coding, parallel code reviews and tests, each stage a separate agent in its own tab of one worktree (parallel stages side by side in one tab), handing results forward as files, with human approval gates. Use when the user says "crew" or asks to run work through the crew / staged multi-agent pipeline. Works from any agent (claude, codex, pi, devin, gemini) running inside a herdr pane.
---

# crew

You are the **orchestrator**. You never do stage work yourself: you start one
agent per stage, hand each one the previous stages' result files, check what
comes back, and ask the user at gates. Every stage runs in its own pane of one
git worktree (its own tab, or side by side with its `parallel` group), so the
user can watch or step into any of them.

All control goes through the `herdr` CLI. If the herdr skill is not already
in your context, run `herdr --skill` and follow its rules — they override
anything here.

## 0. Preconditions

```sh
test "${HERDR_ENV:-}" = 1   # otherwise: say you are not inside herdr and stop
```

Read the stage definition: `crew.md` next to this file
(`<repo>/.agents/skills/crew/crew.md`). The user may have edited it — follow
it as written. Each `## <stage-id>` section has:

- `kind` — `self` (your own agent kind) or a herdr kind: `claude`, `codex`,
  `pi`, `devin`, `gemini`, …
- `gate` — `yes` means stop and ask the user after this stage
- `parallel` — stages with the same value, listed next to each other, run
  at the same time
- `retry` — which stage receives the feedback when this one fails
  (default: itself)
- the paragraph below — that stage's role

Resolve `self` from your own pane:

```sh
herdr agent get "$HERDR_PANE_ID"    # use the agent kind it reports
```

If a named kind is not installed (`command -v <kind>` fails), tell the user
before starting and ask which kind to use instead.

## 1. Confirm the run

Pick a short slug from the goal (`[a-z0-9-]`, at most 16 chars). Show the
user, in one short message: the goal, branch `crew/<slug>`, and the stage
list with resolved kinds and gates. Start only after they agree.

## 2. Worktree and handoff directory

```sh
herdr worktree create --cwd "$PWD" --branch crew/<slug> --label crew-<slug> --no-focus
```

From the JSON take the workspace ID, the root pane ID and the checkout path.
Uncommitted changes in the source checkout are not copied — mention it if
`git status --porcelain` is non-empty.

Results go to `<checkout>/.crew/<slug>/NN-<stage-id>.md` (NN = 01, 02, … in
stage order). Keep them out of commits:

```sh
mkdir -p <checkout>/.crew/<slug>
echo '.crew/' >> "$(git -C <checkout> rev-parse --git-common-dir)/info/exclude"
```

## 3. Run each stage

**Location.** One tab per step, where a step is a single stage or a
`parallel` group. The stages of a group share one tab, split side by side,
so the user can watch them together.

- The first step uses the worktree's root pane. Every later step gets a new
  tab, labelled with the stage id, or with the group name for a group:

  ```sh
  herdr tab create --workspace <ws> --cwd <checkout> --label <stage-id|group> --no-focus
  ```

  The first stage of the step uses the returned root pane.
- Each further stage of a group splits the previous stage's pane to the
  right and uses the new pane:

  ```sh
  herdr pane split <prev-pane-id> --direction right --cwd <checkout> --no-focus
  ```

A stage that runs again (retry, or a rerun after one) reuses its agent
and pane. Never open a second tab or pane for it.

**Agent.** Name it `<slug>-<stage-id>` (lowercase, at most 32 chars,
`[a-z][a-z0-9_-]*`):

```sh
herdr agent start <name> --kind <kind> --pane <pane-id>
```

**Prompt.** One message containing, in this order:

1. The stage role paragraph from `crew.md`.
2. `Do this stage yourself in this pane. Do not spawn subagents or
   background agents.` — the crew already gives each stage its own agent
   in a visible pane; hidden subagents duplicate that and return summaries
   instead of first-hand reads.
3. `Goal: <the user's request>`
4. `Inputs:` the result files of all earlier stages, as absolute paths.
   Pass paths only, never paste their contents or terminal output.
5. `Write your complete result to <absolute result path>. The first line
   must be exactly "verdict: pass" or "verdict: fail". Then reply "done".`

```sh
herdr agent prompt <name> "<prompt>" --wait --timeout 1800000
```

**Parallel stages.** Start and prompt each one without `--wait`, then
`herdr agent wait <name> --timeout 1800000` for each.

**Timeouts and stalls.** A timeout does not mean the prompt was lost. Run
`herdr agent get` / `herdr agent read <name> --source recent-unwrapped --lines 80`,
then wait again. Never resend the same prompt.

**Blocked.** If an agent is `blocked` (approval or question dialog), read
it and ask the user. Never answer approvals yourself.

## 4. Check the result

After the agent settles:

- **File missing.** Read its screen, then send one follow-up asking it to
  write the result to the path. If it is still missing, stop and ask the user.
- **`verdict: pass` and `gate: no`.** Continue to the next stage.
- **`verdict: pass` and `gate: yes`**, or **`verdict: fail`.** Stop and ask
  the user, with a 3–5 line summary of the result file and its path:
  - **continue**
  - **retry** — with the user's feedback (and, on fail, the failing
    result's path). Prompt the `retry` stage's existing agent to fix and
    overwrite its result file, then rerun every stage after it.
  - **stop**

## 5. Finish

Report a short table: stage, agent kind, verdict, result path. Then the
branch and workspace. Leave the agents, tabs and panes in place so the user can
inspect them.

Do **not** merge, push, remove the worktree or close tabs unless the user
asks. Do **not** stop or restart the herdr server.
