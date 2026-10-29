# Epic flow walk

Run one pass per epic when any child has a non-empty `demo_statement`:

1. Use `task action=dep_list id=<epic-id>` to enumerate ParentChild children;
   fetch each with `task action=show`. Include closed children. Record every
   non-empty demo with its child ID; if none exist, skip the walk.
2. Assemble the final epic tip in its dedicated worktree. Give the walk a
   binary of that tip, because the evidence agent cannot compile behind the
   worker build guard. Either build it yourself and place it at
   `<artifacts_root>/<project-key>/<epic-id>/candidate/`, or sequence the
   binary cells after publish against the installed release whose `--version`
   names the tip. Record its path, `--version` and SHA-256 for the ledger.
   Launch the release gate detached using the project's release procedure
   **before** dispatching QA. Record the gate receipt and schedule its
   reminder immediately; neither QA availability nor completion may block the
   gate launch or monitoring.
3. Inspect `<artifacts_root>/<project-key>/<epic-id>/LEDGER.md` (`[factory] artifacts_root`) and epic notes before spawning.
   If a pass is running or complete, resume its recorded agent or consume its
   receipt; never spawn a duplicate on a gate retry or supervisor restart.
   Otherwise initialize that ledger with status `running`, epic tip, worktree,
   binary source, gate receipt, start/deadline, and all child demos. Spawn
   exactly one verifier-class evidence agent against that assembled build,
   concurrently with the gate, and record its agent ID in the ledger. This is evidence
   collection, not a close verdict: use a separate evidence-agent prompt with
   the procedure below, not the sealed task-verifier close dispatch.
4. Give the agent `cas-qa-craft` and the epic ledger context. Override its task
   scope and 30-minute defaults with one combined matrix and a **60-minute**
   total time box. Row 1 is the combined demo happy path; map every child demo
   to explicit actions/expectations in that row or another cell. Keep the same
   quotas across the whole matrix: at least three unmentioned conditions, at
   least one adjacent surface, zero replay cells after row 1, cap **8 cells**.
   Combine related child actions without dropping coverage; name any coverage
   that cannot be exercised within the cap as owed work in Honesty.
5. Keep the QA row grammar, labels, capture requirements, constants grep, and
   defect ownership unchanged. Apply the multi-surface contradiction rule
   across children: dump visible terminal-state text from every affected
   surface; a surface fixed by one child must not contradict another child's
   surface. Map conflicting claims to child IDs and captures in Contradictions.
   This combined-tip check catches regressions that each child's own QA misses.
6. At the deadline mark all unrun cells `NOT EXERCISED: <reason>`; keep them as owed
   work. File one task per defect without patching the build. Finish the single
   ledger with status `complete` and add exactly one epic note headed
   `Epic flow walk` with tip, child coverage, ledger path, counts, and label
   split. Stop registered QA servers. A completed pass may still contain FAIL
   or NOT EXERCISED; completion is not approval.
7. Once both gate and walk receipts exist, call `task action=close id=<epic-id>`
   as the registered epic owner. With verification enabled and deep/default
   depth, close returns the pending dispatch ID and bound head; ownership and
   assembly proof do not replace an epic verdict. Use that exact dispatch for
   the sealed task-verifier review (`verification_type=epic`), or an authorized
   registered supervisor-direct verdict, for example:

   `verification action=add task_id=<epic-id> dispatch_id=<dispatch-id> verification_type=epic status=rejected summary="<evidence verdict>"`

   Use `status=approved` only after accepting the evidence. Require the evidence note and task-verifier
   Step 0A REJECT table before reading the close reason. If the assembled tip
   changes, the receipt is stale: record `status=rejected` against the active
   dispatch, report the mismatch and owed work, and retain genuine deployment
   evidence; do not silently rerun this once-per-epic pass. After an approved
   verdict retry close. After rejection, keep the epic open until the owed
   evidence is resolved; the next close creates a fresh review dispatch.

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
Budget: 60 minutes; status: complete
Owed: M04 keyboard revisit (time box expired).
Contradictions: none in captured terminal states; M04 remains unexercised.

| id | cell | expected | observed | verdict | label | evidence path | defect task |
| --- | --- | --- | --- | --- | --- | --- | --- |
| M01 | combined demo happy path | both child actions complete | both actions completed | PASS | real-build | M01.txt | - |
| M02 | invalid input | clear refusal | clear refusal shown | PASS | real-build | M02.txt | - |
| M03 | adjacent status | count agrees with list | count and list agree | PASS | real-build | M03.txt | - |
| M04 | keyboard revisit | focus and state persist | time box expired | NOT EXERCISED: time box expired | source-inferred | - | - |
```
