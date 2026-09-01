# Awaiting-merge triage

Use this reference when a worker hands off a task for merge. The queue is a
visibility and handoff list; the canonical review procedure is in
`workflow.md` Phase 3, step 5.

```bash
cas__task action=list status=awaiting_merge
```

For each task in the queue:

1. Read the task spec, acceptance criteria, worker commit, and proof notes.
2. Inspect the direct diff against the spec and check ownership boundaries,
   tests, and delivery receipts.
3. Merge the worker branch into the epic, then re-run touched-module tests on
   the merged tree.
4. Record the review receipt after the checks pass:
   `cas__verification action=add task_id=<task-id> verification_type=task status=approved summary="<merged-tree diff vs spec; ownership, tests, receipts, and touched-module proof checked>" files="<changed-files>"`.
5. If the review or proof fails, do not approve the merge. Create a bounded
   fix task or use `request_changes`, then repeat the triage after correction.

The final epic gate runs full nextest on the assembled tree after all queued
tasks have been reviewed and merged.
