# task-verifier close-reason regression fixture (cas-7c37)

Manual policy fixtures for `cas-cli/src/builtins/agents/task-verifier.body.md`.
When the agent prompt changes, re-run these against a scratch verifier session
and confirm the expected verdict.

## Case 1 — ACCEPT (OpenClaw repro)

**Task AC**: "Daemon upgraded to 2026.4.8, Slack probe returns green, Signal
user removal verified."

**Close reason**:
> Daemon upgraded to 2026.4.8. Slack probe green. Signal confirmed user-removed
> pre-upgrade pending dedicated bot number — runbook updated to match.

**Expected**: APPROVE. Every AC item is described as satisfied. "pending
dedicated bot number" is a forward-looking roadmap note that does not belong
to this task's AC. Rejecting on keyword "pending" was the bug cas-7c37 fixes.

## Case 2 — ACCEPT (follow-up note)

**Task AC**: "Migrate 12 users from legacy auth to new SSO."

**Close reason**:
> Migrated 12 users. Follow-up: monitor for 24h and file any edge cases
> separately.

**Expected**: APPROVE. Migration complete; the monitoring follow-up is out of
this task's scope.

## Case 3 — REJECT (actual AC gap)

**Task AC**: "Implement login and logout flows for OAuth provider."

**Close reason**:
> Partially implemented auth; login works but logout still broken.

**Expected**: REJECT. The close reason explicitly says an AC item (logout) is
not done. Category: `incomplete_close_reason`.

## Case 4 — REJECT (stub admission)

**Task AC**: "Implement `compute_tax()` per state table."

**Close reason**:
> Stubbed `compute_tax()` to return 0 for all states; foundation for future
> work.

**Expected**: REJECT. "Stubbed" + "foundation for future work" directly admits
the AC is not implemented.

## Cases 5–8 — finding severity and reproduced failure (GH #1341)

All nominal acceptance criteria and happy-path tests are reported satisfied.
The reviewer has the finding below; a follow-up being filed does not remove it.
File locations here are synthetic fixture inputs, not claims about this repo.

| Case | Reviewer finding | Expected tool verdict |
| --- | --- | --- |
| 5 | **high**, `src/detector.ts:42`: real-PG replay under `America/Chicago` detects zero rows in 5/5 cases, silently; UTC passes. The task is to detect recent incidents. Worker calls the timezone bug non-blocking and files a follow-up. | `status=rejected`, summary starts `REJECTED`, issue `severity=blocking`; name the condition, replay result and lost incident detection. |
| 6 | **medium**, `src/reclaim.rs:42`: terminate the collector after its first unlink, then run GC. The task's own reclaim leaves target bytes permanently unreachable with no report; the marker was removed before the payload. | `status=rejected`, summary starts `REJECTED`, issue `severity=blocking`; the reproduced realistic silent failure overrides the impact label. |
| 7 | **medium**, `src/report.ts:42`: redundant read adds 10ms to a report. The correct result is still shown, within the task's latency bound; no silent failure was reproduced. | `status=approved`, summary starts `APPROVED`, issue `severity=warning`; this improvement can remain non-blocking. |
| 8 | **critical**, `src/auth.ts:42`: source review finds an exposed production credential. A live exploit was not run. | `status=rejected`, summary starts `REJECTED`, issue `severity=blocking`; high-or-above impact is sufficient without a reproduced failure. |

Case 5 is adapted from the reported non-UTC detector failure in GH #1341;
case 6 from cas-2f3b's interrupted cleanup. The v58 merge-then-reject history
(cas-2f3b, cas-eb7b, cas-d2cb) motivates the policy; these fixture rows do not
claim fresh execution of those incidents.

For case 5 the recorded call must retain the existing grammar (normal dispatch
binding still applies): `verification action=add task_id=<fixture-id>
status=rejected summary="REJECTED: incident detection silently fails under
America/Chicago" issues='[{"file":"src/detector.ts","line":42,"severity":"blocking","category":"correctness","problem":"High impact: zero incidents detected in 5/5 non-UTC replays","suggestion":"Normalize the time boundary and rerun the non-UTC replay"}]'`.
Never send `severity=high`: impact is the reviewer's rating, while the tool's
issue severity remains `blocking` or `warning`.

## How to run

Paste each close reason into a scratch `mcp__cas__task action=close` call on a
throwaway task whose AC matches the stated AC, observe the verifier subagent's
verdict, compare to expected. Or manually invoke the task-verifier agent with
the close reason and task context and read its summary.
