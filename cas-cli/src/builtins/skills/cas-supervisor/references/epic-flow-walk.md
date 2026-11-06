# Epic flow walk

In qa.mode=batch (the default) the release's batch QA round at the frozen tip is the only walk; per-task QA evidence applies only in qa.mode=per_task. That round walks an epic whose children carry a non-empty `demo_statement` this way:

1. List ParentChild children with `task action=dep_list id=<epic-id>` and
   fetch each with `task action=show`, closed ones included. Record every
   non-empty demo with its child ID; with none, skip the walk.
2. Assemble the frozen tip in its dedicated worktree. Build the walk's binary
   of that tip yourself (the evidence agent cannot compile behind the worker
   build guard) at `<artifacts_root>/<project-key>/<epic-id>/candidate/`.
   Record its path, `--version` and SHA-256 for the ledger.
3. Inspect `<artifacts_root>/<project-key>/<epic-id>/LEDGER.md` (`[factory] artifacts_root`) and the epic notes first.
   If a pass is running or complete, resume its agent or consume its receipt;
   never spawn a duplicate after a supervisor restart. Otherwise initialize the
   ledger with status `running`, epic tip, worktree, binary source,
   start/deadline and every child demo, then spawn exactly one verifier-class
   evidence agent against the assembled build and record its agent ID. This is
   evidence collection, not a close verdict: give it its own prompt with this
   procedure, not the sealed task-verifier close dispatch.
4. Split each channel/viewport into an independent sub-lane or use the pinned kit's
   `runSurfaceLanes` jobs; give each its own persona, browser profile and ledger.
   Give the agent `cas-qa-craft` and the ledger context, with one combined
   matrix time-boxed by `qa.pass_timeout_mins` in place of its per-task
   defaults. Row 1 is the combined demo happy path; map every child demo to
   explicit actions in that row or another cell. Across the matrix: at least
   three unmentioned conditions, at least one adjacent surface, zero replay
   cells after row 1, cap **8 cells**. Name coverage that cannot fit as owed
   work in Honesty.
5. Keep the QA row grammar, labels, captures, constants grep and defect
   ownership. Dump visible terminal-state text from every affected surface: a
   surface fixed by one child must not contradict another child's. Map
   conflicting claims to child IDs and captures in Contradictions.
6. At the deadline, mark unrun cells `NOT EXERCISED: <reason>` as owed work.
   List each finding with severity and reach; keep theoretical findings in the backlog and staff main-journey FAILs first ([triage](workflow.md#phase-1-plan)).
   Finish the ledger with status `complete` and add exactly one epic note headed
   `Epic flow walk` with tip, child coverage, ledger path, counts and label
   split. Stop registered QA servers. A complete pass may still hold FAIL or
   NOT EXERCISED; completion is not approval.
7. Once the `batch_receipt` exists and the epic is integrated, call `task action=close id=<epic-id>`
   as the registered epic owner. With verification enabled, close returns the
   pending dispatch ID and bound head; ownership and assembly proof do not
   replace an epic verdict. Use that dispatch for the sealed task-verifier
   review (`verification_type=epic`), or an authorized supervisor-direct
   verdict, for example:

   `verification action=add task_id=<epic-id> dispatch_id=<dispatch-id> verification_type=epic status=rejected summary="<evidence verdict>"`

   Use `status=approved` only after accepting the evidence, and read the
   evidence note and task-verifier Step 0A REJECT table before the close
   reason. If the tip changes beyond the one batch-fix re-walk, the receipt is
   stale: record `status=rejected` on the active dispatch, report the mismatch
   and owed work, and keep genuine deployment evidence; never rerun the whole
   walk. After approval, retry close; after rejection, keep the epic open until
   the owed evidence is resolved, and the next close opens a fresh dispatch.

## Example epic note and ledger

Illustrative receipt and matching matrix, not proof of an actual run. Copy the
whole block to the ledger and supply the named captures before checking it:

```text
Epic flow walk
Tip: <assembled-commit-sha>; worktree: <dedicated-epic-worktree>
Children: cas-1111 M01/M02; cas-2222 M01/M03/M04
Ledger: <artifacts_root>/<project-key>/<epic-id>/LEDGER.md
cells=4; PASS=3; FAIL=0; NOT EXERCISED=1
labels: source-inferred=1; fixture=0; real-build=3; eyewitness=0
Budget: qa.pass_timeout_mins; status: complete
Owed: M04 keyboard revisit (time box expired).
Contradictions: none in captured terminal states; M04 remains unexercised.

| id | cell | expected | observed | verdict | label | evidence path | defect task |
| --- | --- | --- | --- | --- | --- | --- | --- |
| M01 | combined demo happy path | both child actions complete | both actions completed | PASS | real-build | M01.txt | - |
| M02 | invalid input | clear refusal | clear refusal shown | PASS | real-build | M02.txt | - |
| M03 | adjacent status | count agrees with list | count and list agree | PASS | real-build | M03.txt | - |
| M04 | keyboard revisit | focus and state persist | time box expired | NOT EXERCISED: time box expired | source-inferred | - | - |
```
