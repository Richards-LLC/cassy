---
from: quick-pelican-89 (Woodworking factory supervisor)
date: 2026-08-02
priority: medium
cas_version: 2.38.2
session: Woodworking-jolly-octopus-76
---

# BUG: worker merge-request messages arrive after the merge, on nearly every task

## What happens

The standard close cycle is:

1. Worker `task close` → MERGE REQUIRED, task parks `awaiting_merge`.
2. Worker sends the supervisor a "please merge `<branch>` at `<tip>`" message.
3. Supervisor merges, then messages the worker to re-close.
4. Worker re-closes.

Steps 2 and 3 are not ordered relative to each other. In this session the supervisor had **already merged and already sent the re-close instruction** before the worker's merge request arrived, on roughly a dozen of ~20 task closes — including cas-2957, cas-b0c0, cas-79f2, cas-32c0, cas-6fac, cas-245d, cas-be95, cas-05e6, cas-247c, cas-a306, cas-340d, cas-bdcb and cas-a66e.

Representative sequence (cas-b0c0), as the supervisor saw it:

```
[supervisor] merged 6a0fe15 → ab94157, messaged worker to re-close
[worker]     "Fresh after draining unread inbox messages until No unread
              messages: tip 6a0fe15 is pushed... please merge into main"
[worker]     cas-b0c0 closed successfully after merge ab94157
```

The worker states it drained its inbox to "No unread messages" immediately before sending, so from its side the request was fresh. The merge notice was evidently not yet visible to it.

## Why it matters

- **Wasted round trips.** Each stale request is a message the supervisor must recognise as already-handled and either ignore or answer redundantly. At ~12 occurrences that is a meaningful fraction of coordination traffic.
- **Ambiguity for the supervisor.** A "please merge X" arriving after merging X is indistinguishable, without checking git, from a *second* push that genuinely needs merging. Every one had to be verified against `git log`/`merge-base` to rule that out.
- **Misleading transcripts.** The task log reads as though the supervisor ignored repeated merge requests.
- **It defeats the freshness protocol.** CAS's own MERGE REQUIRED text instructs workers to drain the inbox to empty before escalating, precisely to avoid stale requests. Workers followed that instruction and the requests were still stale, so the protocol is not achieving its purpose.

## Probable cause

The `awaiting_merge` parking, the worker's outbound message, and the supervisor's inbound merge notice appear to be delivered through paths with no shared ordering guarantee. The inbox-poll "at-most-once" semantics documented in the MERGE REQUIRED remediation text are consistent with a worker polling clean and then receiving the notice after it has already composed and sent.

## Suggested fixes, cheapest first

1. **Make the merge state authoritative at send time.** Before delivering a worker→supervisor merge request, re-check reachability of the named tip against the task's target branch; if it is already an ancestor, drop the message and instead notify the worker that the merge landed. This alone would eliminate nearly all the noise.
2. **Stamp merge requests with the branch tip and target branch tip** (the former is already there; the latter is not), so a supervisor can tell staleness at a glance without running git.
3. **Have the merge itself push a notification to the assignee** as a first-class lifecycle event rather than relying on a supervisor-authored message, so the worker learns of the merge through the same channel that carries task state.
