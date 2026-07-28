# BUG: `worker_status` STALLED flag is a false positive — `last activity` ignores the worker CLI's own tool calls

**Filed:** 2026-07-28
**Reporter:** supervisor `quick-condor-54`, factory session `Penguinz-keen-newt-48` (project: Penguinz, host: soundwave)
**Severity:** High — the flag tells the supervisor to kill healthy workers.

## Summary

`coordination action=worker_status` reported a codex worker as
`⚠ STALLED (no activity ≥300s while task in progress)` while that worker was
executing 2–9 tool calls **per minute, continuously, with no gap longer than
~60 seconds**. The `last activity` timestamp had frozen ~6 minutes earlier and
never advanced, even though the worker was shelling out constantly and had just
written a file.

The stall detector is the factory's only push-ish signal that a worker is wedged.
Because it is driven by a signal that excludes the worker's actual work, it
reports healthy investigation-heavy workers as dead.

## Reproduction (as observed)

Worker `vivid-panther-55` (`cli=codex`, `model=gpt-5.6-sol`, `effort=medium`,
`isolate=true`) on task `cas-b0b4` — a diagnosis/characterization spike that is
almost entirely `exec_command` shell work (nvidia-smi traces, journalctl reads,
py-spy/gdb stack sampling) and deliberately makes **no repo commits and few CAS
MCP calls** for long stretches.

1. Assign a worker a read-only investigation task (no commits, no CAS MCP calls
   for several minutes; pure shell diagnostics).
2. Wait ~6 minutes.
3. Run `coordination action=worker_status`.

**Expected:** worker shown as active; `last activity` tracks its tool calls.
**Actual:** `last activity: 366s ago (activity) ⚠ STALLED (no activity ≥300s while task in progress)`.

## Evidence

`worker_status` at `12:09:32Z`:

```
• vivid-panther-55 (heartbeat: 18s ago)
  last activity: 366s ago (activity) ⚠ STALLED (no activity ≥300s while task in progress)
  session: codex-vivid-panther-55-ec18396e-d887-4c23-a64d-2c487e5eedf8
```

`366s ago` backdates the last recorded activity to ≈ `12:03:26Z`.

Ground truth from the worker's own codex rollout
(`~/.codex/sessions/2026/07/28/rollout-2026-07-28T07-59-08-019fa897-cb68-7c70-b18e-68ae589a62ca.jsonl`),
events bucketed per minute:

```
minute | events | tool_calls
 11:59  |    55  |  8
 12:00  |    20  |  5
 12:01  |    18  |  4
 12:02  |     9  |  2
 12:03  |    23  |  6
 12:04  |     4  |  1     <-- CAS believed the worker died here
 12:05  |     8  |  2
 12:06  |     8  |  2
 12:07  |     8  |  2
 12:08  |     8  |  2
 12:09  |    14  |  3     <-- worker_status says "STALLED" at 12:09:32
 12:10  |    41  |  9
 12:11  |    17  |  4
```

Never a minute with zero tool calls. Corroborating independent evidence over the
same window:

- The worker drove a full 30-step SDXL generation through the A1111 API at
  `12:02:21–12:02:27Z` (visible in `journalctl --user -u a1111`).
- At `12:11:09Z` it performed an `apply_patch` creating
  `.cas-b0b4-memory-summary.py` in its worktree — i.e. a **file edit**, which
  `worker_activity`'s own help text claims is tracked.
- OS process alive throughout: `pid 870319`, state `SNl+`, ~5% CPU, 33s
  cumulative CPU over 700s elapsed (consistent with streaming + shell work).
- Its only child was `cas serve` — no hung subprocess, so nothing external was
  blocking it.

## Suspected root cause

`last_activity` appears to be updated only by CAS-side events (CAS MCP tool
invocations, and/or git-observable commits), not by the worker CLI's own
tool-call stream (`exec_command`, `apply_patch`, shell work).

Supporting contrast in the same session: the sibling worker `ready-phoenix-39`,
doing an audit task that called CAS MCP tools frequently (task notes, blocker
notes), showed a healthy `last activity: 50s ago` throughout — while
`vivid-panther-55`, doing pure shell diagnostics, froze. Same CLI, same model,
same spawn parameters; the only difference is how often they touched CAS MCP.

This makes the metric a proxy for "how chatty is this worker with CAS", not
"is this worker doing work".

## Impact

1. **The documented recovery action is destructive.** The supervisor guidance
   treats a stalled worker as a recovery candidate (shutdown/reset/respawn). Acting
   on this false positive kills a healthy worker. In this session it would have
   aborted a live `gdb`/`py-spy` capture of a transient, hard-to-reproduce
   GPU stall — perishable evidence that is expensive to recreate.
2. **It burns supervisor context.** Disproving the flag took ~10 minutes and a
   dozen tool calls (`nvidia-smi`, `py-spy dump`, `ps --forest`, journal reads,
   parsing the codex rollout JSONL by hand). None of that is work the supervisor
   should have to do, and the transcript cost is real.
3. **It inverts the trust model.** After one false positive, a supervisor
   rationally starts ignoring STALLED — which is exactly when a real wedge slips
   through. A liveness signal that cannot be trusted is worse than none.
4. **It penalises exactly the tasks that need it least.** Investigation, spike,
   audit, and characterization tasks are the most shell-heavy and least
   commit-heavy, so they are the most likely to be falsely flagged.

## Requested fix

Primary:

- Update `last_activity` from the worker CLI's tool-call stream (any
  `function_call` / `custom_tool_call` / `exec_command` / `apply_patch` event),
  not only from CAS MCP calls and commits. The rollout JSONL is already on disk
  and already timestamped; its mtime alone would be a strictly better signal than
  what is used today.
- Until that lands, do not render `⚠ STALLED` from this metric, or downgrade its
  wording to something that cannot be mistaken for a liveness verdict (e.g.
  `no CAS interaction for Ns — not a liveness signal`).

Secondary (same root cause, surfaced together):

- `coordination action=worker_activity` returned **"No recent worker activity"**
  while two workers were actively executing tool calls. Its help text says
  activity is tracked "when workers edit files, run subagents, or commit code" —
  but a worker that had just `apply_patch`ed a file still did not appear. Either
  the tracking is narrower than documented, or the docs are wrong; both should be
  fixed. As-is, `worker_activity` cannot corroborate or refute a STALLED flag,
  which is precisely what a supervisor reaches for next.

## Related defect found while diagnosing this (separate, small, real)

`coordination action=message` returns:

```
ID: 590
...
Check `message_status` (id above) if you need to know whether this landed before escalating.
```

but `coordination action=message_status id=590` fails:

```
MCP error -32602: notification_id required for message_status (the prompt queue message ID)
```

The response text tells the caller to use the ID it just printed, and the tool
rejects it. Hit **twice independently in this session** — once by the supervisor
(`12:05:45Z`) and once by worker `ready-phoenix-39` (`12:05:06Z`), both logged as
`event: error` in `.cas/logs/factory-session-2026-07-28.log`.

This matters here because "did my message actually land?" is the supervisor's
documented next step when a worker looks silent — i.e. the escalation path is
broken exactly in the scenario this bug report is about.

Fix: accept the message ID returned by `action=message`, or correct the response
text to name the parameter and ID the tool actually wants.

## Environment

- CAS factory session `Penguinz-keen-newt-48`, supervisor CLI `claude`, worker CLI `codex`
- codex-cli `0.145.0`, `--model gpt-5.6-sol -c model_reasoning_effort=medium`
- Host soundwave, Kubuntu 26.04, kernel 7.0.0-28
- Logs: `.cas/logs/factory-session-2026-07-28.log`, `.cas/logs/cas-2026-07-28.log`

---

## Resolution — 2026-07-28

All three defects in this report are fixed, plus a fourth found while diagnosing them.
Delivered under epic **cas-d6f1**; verified on the assembled epic (full workspace gate green,
real-PTY runtime tests green with default flags).

| Reported | Task | Fix |
|---|---|---|
| Frozen `last activity` / false ⚠ STALLED | **cas-fa69** | `transcript_path_fast` was Claude-only, so Codex workers always resolved to `None` and the cas-a653 fix was unreachable. `worker_status` now uses the cli-aware resolver. |
| — (found during the fix) | **cas-479f** | A `cas-codex-exec` shell-out creates a second rollout in the same cwd, making resolution permanently `Ambiguous`. Exec rollouts are now ignored when resolving a worker's transcript. Without this, cas-fa69 was inert for exactly the shell-heavy investigation workers this report describes. |
| `worker_activity` blind to active workers | **cas-a568** | Now consumes the same corrected transcript signal as `worker_status`; no second detector introduced. |
| `message_status` rejects the ID `message` prints | **cas-0440** | The send response now labels the value `notification_id` and gives the exact follow-up syntax. One spelling, no alias. |

Net effect on the original complaint — that a supervisor had no trustworthy instrument to
distinguish a wedged worker from a working one: `worker_status`, `worker_activity` and
`cas factory is-wedged` now consume the same evidence.

**Note for operators:** the fixes take effect only once the `cas` binary is rebuilt and installed.
