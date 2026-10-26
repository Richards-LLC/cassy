---
name: finish
description: Use when the operator asks to finish or wrap up a session, clean up session work, and save its handoff and learnings.
metadata:
  managed_by: cas
---

Wrap up this session. Perform every applicable step below; the explicit finish request authorizes these actions. Report the results in a short table.

1. **Workers** (factory supervisors only). Run `factory action=worker_status`. Merge or park ready work with green CI. Shut down idle workers with `factory action=shutdown_workers worker_names=<names>`; name any worker kept alive and why. Preserve workers needed for pending tasks or review.
2. **Files and branches.** Remove only scratch you created this session. Resolve its location with `cas config get factory.scratch_root`; preserve durable artifacts under the configured `factory.artifacts_root` and anything cited by task notes. Delete remote branches you created only after `git merge-base --is-ancestor` proves they are fully merged into the default branch. Supervisors may run `factory action=gc_report` and clean up items they own. Leave the working tree as you found it, or list what remains and why.
3. **Handoff.** Save one `memory action=remember entry_type=handoff`. It replaces earlier handoffs, so include what shipped (versions and SHAs), work in flight with its exact next action, open operator decisions, and standing operator rules from this session.
4. **Memories.** Save new operator preferences and decisions as `entry_type=preference` or `context`. Search first and update existing entries instead of duplicating them.
5. **Learnings.** Save each non-obvious lesson as `entry_type=learning`: what went wrong, why, and the rule that prevents it. If enforcement is needed, file a task to put it in code.
6. **Tasks.** Record every open thread as a task or an explicit note on one, so nothing remains only in chat.
7. **Report.** Summarize cleanup, the handoff memory ID, saved memory and learning IDs, and anything deliberately retained.
