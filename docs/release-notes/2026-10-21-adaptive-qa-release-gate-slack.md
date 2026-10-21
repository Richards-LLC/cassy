# Draft release-note additions — adaptive QA release gate

Channel: #cas-internal (`C0B44GUKDK2`). Draft only; not posted.

This draft belongs to the runtime cut that ships the adaptive QA release gate.
The release owner adds that cut's version, other changes, install receipts and
publication timing using the [runtime release template](runtime-release-template.md)
before publication. The versionless labels below are valid draft labels.

## User thread

Top-level:

```text
*Live on production — User — Cassy*
Was: a stalled check could pass unnoticed. → Now: timeouts raise alerts and deployment failures identify what needs attention.
```

Reply:

```text
• *Stalled checks raise an alert* — Was: a check that ran out of time could finish without a failure alert.
→ Now: a timed-out check raises the same alert as a failed check, naming the job that needs attention.

• *One reminder after upgrading* — Was: an already-reported failure stayed quiet through an update.
→ Now: the first upgrade to this release can repeat one alert for each still-failing branch. Later polls stay quiet for that same failure.
```

## Dev thread

Top-level:

```text
*Live on production — Dev — Cassy*
Was: CI timeouts were silent and deploy diagnostics were generic. → Now: timeouts count as red and alerts identify each failed workflow.
```

Reply:

```text
• *CI timeouts count as red* — Was: only a completed CI run with `failure` was relayed.
→ Now: `timed_out` also counts as red at the branch tip when GitHub reports a failed or timed-out job. Cancelled-only jobs remain silent.

• *Workflow-specific dedupe* — Was: CI alerts used `ci-red-run:<branch>:<sha>`.
→ Now: keys include the workflow as `ci-red-run:<branch>:<workflow>:<sha>`. On the first upgrade, each watched branch whose latest CI run is still red at its tip can alert once again, even if the old key was already reported.

• *Deployment diagnostics* — Was: deploy alerts said “CI failed” and identical observations rewrote local state on every poll.
→ Now: alerts name the failed deploy workflow, unchanged observations keep the state file untouched, and repaired read/parse/write errors can warn again if they recur.
```
