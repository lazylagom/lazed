# crew stages

Edit this file to shape the crew for this project. Stages run top to bottom.

Each `## <stage-id>` section takes these fields:

- `kind` — `self` (the orchestrator's own kind) or a herdr kind:
  `claude` | `codex` | `pi` | `devin` | `gemini` | …
- `gate: yes` — the orchestrator stops and asks you after this stage
- `parallel: <group>` — adjacent stages with the same group run at the same time
- `retry: <stage-id>` — which stage gets the feedback when this one fails
  (default: itself)

The paragraph under each heading is that stage's role. Mixing kinds, for
example `kind: codex` on the verify and review stages, gives each check a
different model's eyes.

## research
- kind: self

Survey the codebase for this goal. Collect facts only, with file paths:
the modules, types, data schema, conventions and tests it touches. Also note
the constraints you find. Do not propose a design and do not change any files.

## design
- kind: self

Write a concrete design from the research: the files to change, the
interfaces, the data flow, edge cases and a test plan. Reuse what the
research found before inventing anything new. Do not write code.

## verify
- kind: self
- gate: yes
- retry: design

Assume the design is wrong and try to prove it. Check it against the code
the research points to. Look for missing cases, broken invariants,
transaction and concurrency issues, and simpler alternatives. Use
"verdict: fail" if anything must change before coding.

## coding
- kind: self

Implement the verified design in this worktree, following the project's
conventions. Make it build. Add or adjust the tests from the design's test
plan. List the changed files in your result.

## review-arch
- kind: self
- parallel: review
- retry: coding

Review the diff (`git diff` against the branch base) for design conformance,
layering, transaction boundaries and naming. Report only actionable
findings, each with its location. Do not edit files.

## review-safety
- kind: self
- parallel: review
- retry: coding

Review the diff for null and empty handling, error paths, N+1 queries,
resource leaks and security issues. Report only actionable findings, each
with its location. Do not edit files.

## test
- kind: self
- gate: yes
- retry: coding

Run the project's full test suite and linters. Report the commands you ran
and their results. Do not fix code; for each failure, give the test name
and the likely cause.
