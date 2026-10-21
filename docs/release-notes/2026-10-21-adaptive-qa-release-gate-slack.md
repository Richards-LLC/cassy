# Draft release-note additions — adaptive QA release gate

Channel: #cas-internal (`C0B44GUKDK2`). Draft only; not posted.

These are reply additions for the runtime cut that ships the adaptive QA release
gate. The release owner incorporates them into the versioned
[runtime release template](runtime-release-template.md), alongside that cut's
other changes, before publication. The version, install receipts and publication
timing belong to that assembled draft.

## User reply additions

```text
• *Stalled checks raise an alert* — Was: a check that ran out of time could finish without a failure alert.
→ Now: a timed-out check raises the same alert as a failed check, naming the job that needs attention.

• *One reminder after upgrading* — Was: an already-reported failure stayed quiet through an update.
→ Now: the first upgrade to this release can repeat one alert for each still-failing branch. Later polls stay quiet for that same failure.
```

## Dev reply additions

```text
• *CI timeouts count as red* — Was: only a completed CI run with `failure` was relayed.
→ Now: `timed_out` also counts as red at the branch tip when GitHub reports a failed or timed-out job. Cancelled-only jobs remain silent.

• *Workflow-specific dedupe* — Was: CI alerts used `ci-red-run:<branch>:<sha>`.
→ Now: keys include the workflow as `ci-red-run:<branch>:<workflow>:<sha>`. On the first upgrade, each watched branch whose latest CI run is still red at its tip can alert once again, even if the old key was already reported.

• *Deployment diagnostics* — Was: deploy alerts said “CI failed” and identical observations rewrote local state on every poll.
→ Now: alerts name the failed deploy workflow, unchanged observations keep the state file untouched, and repaired read/parse/write errors can warn again if they recur.
```
