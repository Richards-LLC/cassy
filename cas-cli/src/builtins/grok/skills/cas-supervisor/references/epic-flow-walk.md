# Epic flow walk

Run one pass per epic when any child has a non-empty `demo_statement`:

1. Use `task action=dep_list id=<epic-id>` to enumerate ParentChild children;
   fetch each with `task action=show`. Include closed children. Record every
   non-empty demo with its child ID; if none exist, skip the walk.
2. Assemble the final epic tip in its dedicated worktree. Launch the release
   gate detached using the project's release procedure **before** dispatching
   QA. Record the gate receipt and schedule its reminder immediately; neither
   QA availability nor completion may block the gate launch or monitoring.
3. Inspect `~/.cas/artifacts/<epic-id>/LEDGER.md` and epic notes before spawning.
   If a pass is running or complete, resume its recorded agent or consume its
   receipt; never spawn a duplicate on a gate retry or supervisor restart.
   Otherwise initialize that ledger with status `running`, epic tip, worktree,
   gate receipt, start/deadline, and all child demos. Spawn exactly one
   verifier-class evidence agent against that assembled build, concurrently
   with the gate, and record its agent ID in the ledger. This is evidence
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
   This combined-tip check addresses the regressions found in
   https://github.com/Richards-LLC/cassy/issues/759.
6. At the deadline mark all unrun cells `NOT EXERCISED`; keep them as owed
   work. File one task per defect without patching the build. Finish the single
   ledger with status `complete` and add exactly one epic note headed
   `Epic flow walk` with tip, child coverage, ledger path, counts, and label
   split. Stop registered QA servers. A completed pass may still contain FAIL
   or NOT EXERCISED; completion is not approval.
7. Once both gate and walk receipts exist, use the normal sealed epic close
   verification dispatch (`verification_type=epic`). Require the evidence note
   and the task-verifier Step 0A REJECT table before reading the close reason.
   If the assembled tip changes, the receipt is stale: reject it and report the
   mismatch and owed work; do not silently rerun this once-per-epic pass.

### Example epic note

Illustrative receipt, not proof of an actual run:

```text
Epic flow walk
Tip: <assembled-commit-sha>; worktree: <dedicated-epic-worktree>
Children: cas-1111 M01/M02; cas-2222 M01/M03/M04
Ledger: ~/.cas/artifacts/<epic-id>/LEDGER.md
cells=4; PASS=3; FAIL=0; NOT EXERCISED=1
labels: source-inferred=1; fixture=0; real-build=3; eyewitness=0
Budget: 60 minutes; status: complete
Owed: M04 keyboard revisit (time box expired).
Contradictions: none in captured terminal states; M04 remains unexercised.
```
