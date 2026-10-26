---
description: Wrap up the session — clean up workers and files, save the handoff, memories and learnings
---

Wrap up this session now. Do every step that applies, skip steps that don't, and report each one in a short table at the end. Don't ask before doing these; they are pre-approved.

1. **Workers** (factory supervisors only). Run `factory action=worker_status`. Merge or park anything ready (green CI only, never pending). Then shut down every idle worker with `factory action=shutdown_workers count=0`. Name any worker kept alive and why.
2. **Files and branches.**
   - Remove scratch you created this session (for example under `/mnt/rewind/cas/scratch/`); keep anything a task note cites as evidence.
   - Delete remote branches you created that are fully merged into main (`git merge-base --is-ancestor`). Never delete unmerged work.
   - Run `factory action=gc_report` if available, and act on what is clearly yours.
   - Leave the working tree as you found it, or list what is left and why.
3. **Handoff.** Save one `memory action=remember entry_type=handoff`. It replaces earlier handoffs, so make it complete. Cover:
   - what shipped, with versions and SHAs;
   - what is in flight and the exact next action;
   - open decisions waiting on the operator;
   - standing operator rules from this session.
4. **Memories.** Save operator preferences and decisions from this session (`entry_type=preference`, or `context`) that aren't already stored. Search first, and update instead of duplicating.
5. **Learnings.** Save each non-obvious lesson as `entry_type=learning`: what went wrong, why, and the rule that prevents it. If a lesson needs enforcement, file a task to put it in code. A memory alone is not enforcement.
6. **Tasks.** Every open thread is either a task or an explicit note on one. Nothing lives only in chat.
7. **Report.** Give a short summary: what you cleaned, the handoff memory ID, the memories and learnings saved (IDs), and anything left on purpose.
