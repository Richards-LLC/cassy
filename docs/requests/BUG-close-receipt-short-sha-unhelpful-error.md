---
from: quick-pelican-89 (Woodworking factory supervisor)
date: 2026-08-02
priority: low
cas_version: 2.38.2
session: Woodworking-jolly-octopus-76
---

# BUG: close receipt silently requires a full SHA; the rejection does not say so

## What happens

`task close` validates the commit receipt but appears to require the full 40-character SHA. A short SHA is rejected without the error naming the problem, so the worker cannot tell a receipt-format failure from a genuine merge-state failure.

Observed on cas-9a65 (worker solid-lynx-17), which reported:

> "Correction: full task SHA is `5ffbb9f00e8d020c9e46ff4bbf27743bd1f30c16`. My first close attempt used the short SHA and was rejected as an invalid receipt; task remains InProgress. Please merge that commit/branch into the epic, then I will retry close with the full receipt."

The worker's own diagnosis was that the merge had not happened — it had, several minutes earlier. The supervisor had to reply with both full SHAs and an explicit `git merge-base --is-ancestor` result before the close succeeded.

The same confusion recurred on cas-05e6, where the supervisor pre-emptively supplied the full SHA and told the worker why, specifically because a previous worker had been rejected for a short one.

## Why it matters

The failure mode is indistinguishable from MERGE REQUIRED. A worker that gets "invalid receipt" concludes its branch is not merged, sends a merge request for something already merged (see `BUG-stale-merge-request-messages.md`), and parks. The supervisor then has to prove the merge landed rather than simply saying "use the long form".

Short SHAs are also what every ergonomic git command emits by default — `git log --oneline`, `git rev-parse --short`, and CAS's own `epic_status` table all display them — so reaching for one is the natural mistake.

## Suggested fixes

1. **Accept a short SHA** and resolve it via the repository, which is what git itself does. This is the preferred fix; there is no ambiguity risk at these repo sizes and none was reported.
2. Failing that, **say what is wrong** in the rejection text: `receipt must be the full 40-character commit SHA; received 7 characters`. Distinguish it clearly from the merge-state guard so a worker does not conclude its work is unmerged.
3. Consider having `epic_status` and other supervisor-facing output that feeds close receipts show the full SHA, or offer both, since those tables are where the value is usually copied from.
