---
from: ozer factory — session ozer-warm-owl-12, supervisor lively-dolphin-39
date: 2026-07-27
priority: P3
cas_task: (none)
---

# `task reopen` refuses blocked tasks, so there is no documented way to unblock one

Smallest issue of the session, but it sits on a common path: a worker blocks a task, the supervisor removes the blocker, and the obvious verb does not work.

## Symptoms

```
mcp__cas__task action=reopen id=cas-d24b
→ MCP error -32602: Task is already blocked (only closed tasks can be reopened)
```

`reopen` is the natural verb for "this task is not finished, put it back in play", and the error confirms the tool understands the task is blocked — it simply declines to act. Nothing in the error names the verb that does work.

## Concrete evidence

cas-d24b was blocked by a worker against an acceptance criterion that the supervisor subsequently **withdrew** (it required iOS background auto-start, which the worker proved Apple does not permit). With the criterion gone, the block was invalid and the remaining work was unblocked — but the worker had gone idle and could not start a blocked task.

`reopen` refused. `update status=open` worked:

```
mcp__cas__task action=update id=cas-d24b status=open assignee=quick-puma-7
→ Updated task cas-d24b: assignee, status
```

The same shape occurred with cas-93f1, blocked against a device-verification criterion that turned out not to apply once the task was rescoped from an audio defect to a UI change.

Both cases share a cause worth noting: **the blocker was a supervisor-authored acceptance criterion that turned out to be wrong.** Workers were right to refuse; the supervisor then needed a clean way to correct the criterion and resume. That is not an exotic path.

## Workaround applied

`update status=open`. Works, but it is undiscoverable from the error, and it bypasses whatever bookkeeping `reopen` presumably does (reason capture — `reopen` accepts a `reason`, `update` does not, so the audit trail loses why the block was lifted).

## Proposed fix

- a) **Let `reopen` accept blocked tasks (leaned).** `blocked → open` with a captured reason is exactly what `reopen` is for. Closed and blocked are both "not in play"; only one is currently reversible by the verb.
- b) If `reopen` must stay closed-only, name the alternative in the error: "use `update status=open` to unblock". One sentence removes the guesswork.
- c) Consider an explicit `unblock` action taking a reason, so the audit trail records *why* a blocker was lifted. In both cases here the reason was substantive — a withdrawn acceptance criterion — and is currently recorded only because I wrote it into a task note by hand.

---

## Resolution

**Resolved 2026-07-27 — shipped in cas v2.29.0 (`origin/main`).** Task: cas-cd24

`task reopen` now accepts tasks in `blocked` status and captures a reopen reason (`9719ad3`, merge `f1d68fb`).
