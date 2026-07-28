# Slack drafts — v2.33.0 (2026-07-28)

Channel: #cas-internal (C0B44GUKDK2)
Two distinct TOP-LEVEL posts. Not threaded. Post only after the tag is pushed.

STATUS: DRAFT — awaiting operator review. Do not post without explicit approval.

---

## POST 1 — User perspective (top-level)

**Was:** you'd check on a busy worker and CAS would tell you it had stalled — so you'd kill it, and lose the work it was in the middle of.
**Now:** CAS watches what the worker is actually doing, so a working worker reads as working.

- A worker running shell commands, editing files, or investigating for long stretches no longer looks dead just because it hasn't checked in. The "stalled" warning now reflects reality, so it's safe to act on again.
- Messages you send to a worker actually arrive. Previously a message could be reported as delivered and never reach it — leaving you waiting on a worker that was waiting on you, indefinitely.
- Urgent interrupts reach an idle worker. The one tool meant for rescuing a stuck worker used to be affected by the same problem.
- Restarting a session no longer discards messages that were queued for workers about to come back.
- Bringing a worker back under a name you'd used earlier in the session now works instead of silently doing nothing.
- Worker context usage is visible again, so you can see one approaching its limit before it gets there.

---

## POST 2 — Dev perspective (top-level)

**Was:** liveness, delivery, and merge-verification each trusted a signal that couldn't see the thing it was reporting on.
**Now:** each reads the evidence it actually depends on, and fails loudly instead of silently when that evidence is missing.

- **Activity tracking:** worker status resolved its transcript through a Claude-only path, so for Codex it always came back empty — freezing the activity clock and defeating in-flight suppression. Status, activity, and the wedged check now share one harness-aware resolution. A read-only shell-out creating a second rollout in the same directory no longer makes that resolution ambiguous.
- **Message delivery:** the prompt queue could re-select the same undeliverable batch indefinitely, blocking everything behind it. Undeliverable messages now become terminal under a bounded retry, and one stuck target can't hold up delivery to a live one. Retry budgets are measured from the first real attempt, so a long wait before a worker registers no longer consumes them.
- **Startup safety:** the queue's cleanup pass no longer runs before the roster is populated, and it now counts registered agents rather than only attached panes — so restarting with a reused session name doesn't discard queued work.
- **Spawn lifecycle:** a shutdown used to leave a permanent tombstone on the worker name, so any later spawn reusing it was built and silently thrown away. Cancellation is now scoped to the specific in-flight spawn, is logged, and cleans up the worktree it created.
- **Merge verification:** the check that confirms a group of related work is fully merged keyed on a branch derived from whoever was assigned, so a branch reused across two groups could strand an unrelated one. It now keys on each piece of work's own recorded commit — and when no record exists it inspects the live branch rather than treating unknown as verified. That recorded commit is captured from the commit itself, so a later reset or rebase in the same command can't anchor the wrong one.
- **Lease history:** release reasons now live in their own column instead of the transfer-attribution field, with existing rows still readable.
- **Skill references:** reference docs under a managed skill now sync with it. Previously only the skill body updated, leaving projects on whatever reference docs they were first initialized with. Local edits are preserved and reported rather than overwritten.
- **Test isolation:** four separate environment-isolation helpers across two locks were collapsed into one guard, removing a family of parallel-run flakes. Real-PTY tests serialize across binaries via a file lock.

---

## Notes for the poster (not part of either post)

- Rubric compliance checked: no ticket IDs, no epic IDs, no supervisor/worker/director/factory orchestration narration, no task-lifecycle bookkeeping. Worker/session behavior is described as product surface because that IS the product for this audience — the prohibition is on narrating how the work was assigned and executed, not on naming the features.
- Both posts lead with a was → now punch.
- Post as two separate top-level messages, not a parent plus reply.
- This is a runtime release only. No harness-diary content changed, so the three-reply diary thread does NOT apply.
