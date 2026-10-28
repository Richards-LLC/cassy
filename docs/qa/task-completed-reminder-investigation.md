# Task completion reminder investigation — GH #1406

The task-filtered reminder failure has existing fixes in **3.81.0**, retained in **3.84.0 and 3.86.0**. No additional runtime change is justified by the supplied incident. The reported 3613-second lifecycle delay remains historically unconfirmed; version skew alone does not establish its precise cause.

## Incident and version boundary

[GH #1406](https://github.com/Richards-LLC/cassy/issues/1406) reports reminder #124 created for `cas-4791` shortly before its close, no reminder wake, and a lifecycle relay surfaced only after operator input. Its supervisor MCP ran **3.66.0** while the installed runtime and workers ran **3.75.0**. Because the reported wait was roughly two minutes, the one-hour default TTL alone does not explain this incident.

Reminder scanning and wake injection run in the factory daemon; the supervisor MCP handles reminder creation. The incident does not supply the daemon epoch/version. A stale supervisor MCP alone cannot establish which scanner or injection implementation executed.

The installed CLI and this worker's `system action=version` both report `3.86.0 (1bee154 2026-10-28)`. Tests below exercise source revision `d5405693d876de2ab77315f301f65a4e745d50ee`, an epic descendant of the released runtime. They do not replay the original production supervisor.

| Existing behavior | First containing release | Verification |
| --- | --- | --- |
| Task-filtered completed/blocked waits default to no expiry; persisted Closed state fires without a local director event | 3.81.0 | `3a00059ef68d2150ad444313eef90b8d169acd93` |
| One missing reminder owner cannot abort later rows; finite expiry and cancellation notify visibly | 3.81.0 | `fad204a3b87789bff659a8d7d993510778f0c763` |
| Supervisor's own reminder wakes after a recorded turn end despite pane repaints; busy turns retain protection | 3.77.0 | `1e2529075`, `6173edc3e` |
| Installed/running MCP versions are compared against live process epochs and stale sessions are named | 3.73.0 | `0eb2cde75`, `d74ed6694` |

`git merge-base --is-ancestor <commit> v3.84.0`, `v3.86.0`, and the tested source HEAD returned 0 for all five behavioral/detection revisions in `version-ancestry.txt`. The reminder fixes and repaint fix are absent from both incident versions. Detection is absent from 3.66.0 and present in 3.75.0.

## Current real paths and existing regression proof

- [`factory_remind.rs`](../../cas-cli/src/mcp/tools/service/factory_remind.rs#L16) chooses TTL=0 only for filtered task waits; an explicitly supplied finite TTL is respected.
- [`queue_and_events.rs`](../../cas-cli/src/ui/factory/daemon/runtime/queue_and_events.rs#L12471) reconciles each pending reminder independently. At line 12531 it reads persisted Closed/Blocked state, checks session/filter, and calls the normal `fire_reminder` path before expiry. A two-second daemon refresh schedules the store job at line 12010; this does not require a model turn or a new client polling loop.
- `bound_task_reminder_survives_phase_and_observes_remote_close_cas_d8a1` registers through real coordination MCP, ages the wait two hours, verifies one explicit-expiry notice, applies Closed via `update_from_sync` without director events, and observes exactly one daemon-authorized reminder. `ended_reminder_owner_cannot_stall_later_task_waits_cas_d8a1` verifies a preceding deleted owner cannot suppress a later completion/expiry.
- `reminder_wake_tests` and `reminder_dispatch_tests` exercise the actual daemon queue pass and recorded transport writes for an idle supervisor. They retain operator-draft, active-tool and mid-turn protection. An authorized wake or PTY write is not proof that a production model authored a reply.

The original task's published proof `art-5605a3a6` records the existing MCP reminder regression failing before the fix (TTL 3600 versus expected 0), then passing with the same filter. Its original delivery passed 695 selected tests. The corrective proof `art-6d086982` supersedes it for orphan/cancellation/successor findings: its same two-test filter failed 0/2 before the correction, passed 2/2 afterward, then passed 612 affected lib/integration tests. Both artifacts were fetched and integrity-verified for this investigation. No duplicate regression was added.

## Fresh validation

At source revision `d5405693d876de2ab77315f301f65a4e745d50ee`, the capped runner completed the following selection:

```sh
cargo nextest run -p cas --lib -E 'test(bound_task_reminder_survives_phase_and_observes_remote_close_cas_d8a1) | test(ended_reminder_owner_cannot_stall_later_task_waits_cas_d8a1) | test(reminder_wake_tests::) | test(reminder_dispatch_tests::) | test(factory_binary_skew::tests::)'
```

```text
Summary [5.480s] 22 tests run: 22 passed, 8879 skipped
PASS: 22 test(s) passed across 1 harness summary line(s).
test: PASS d5405693d876de2ab77315f301f65a4e745d50ee cas test(bound_task_reminder_survives_phase_and_observes_remote_close_cas_d8a1) | test(ended_reminder_owner_cannot_stall_later_task_waits_cas_d8a1) | test(reminder_wake_tests::) | test(reminder_dispatch_tests::) | test(factory_binary_skew::tests::) 22
worker-check: EXIT 0
```

The requested 22 tests exclude the runner's two safety preflight tests. `current-tests.log` retains the command, individual results and receipt under this task's published evidence. Comparison with `v3.86.0` shows no changes in `factory_remind.rs`, `factory_binary_skew.rs` or the reminder wake/dispatch test files. The only `queue_and_events.rs` difference is the unrelated consumed-candidate filter owned by `cas-ad3d`; the reminder reconciliation and wake functions are unchanged.

The named selection covers the two persisted-task reconciliation regressions plus whole `reminder_wake_tests`, `reminder_dispatch_tests`, and `factory_binary_skew::tests` modules. No production function changed, so broad Rust assembly is not repeated. Full integrated execution remains owned by the merge queue.

## Visible warning and recovery

[`factory_binary_skew.rs`](../../cas-cli/src/factory_binary_skew.rs#L296) renders **“Factory MCP servers older than installed Cassy”**, installed and live server versions, affected names/PIDs, and recovery instructions. [`handlers_session.rs`](../../cas-cli/src/hooks/handlers/handlers_session.rs#L304) inserts this into supervisor SessionStart context; `factory worker_status` exposes the same version snapshot. Post-install `notify_after_install` sends one deduplicated supervisor notice. The existing tests use live child-process epochs and real stores, including stale supervisor names and the post-install notice queue.

The supervisor version check is **`system action=version` in the supervisor's own MCP connection**, compared with **`cas --version`** for the installed executable. A worker version does not establish the supervisor's version. Installation does not replace an already running MCP process. For Claude, the warning names `/mcp` reconnect to preserve workers; restarting the supervisor stops every worker, so worker state must be saved first. No process was restarted by this investigation.

## Limits and adjacent ownership

The originating reminder/relay trace is unavailable in this checkout: its local reminder #124 belongs to an unrelated August session, and no reminder filter names `cas-4791`. Consequently, the historical failure is **INCONCLUSIVE** among stale event dispatch, wake gating and downstream queue delivery; it is not attributed solely to TTL expiry or version skew.

[GH #1393](https://github.com/Richards-LLC/cassy/issues/1393), tracked separately as `cas-f625`, concerns lifecycle delivery windows and superseding task transitions. The supplied GH1406 trace does not establish that it shares the reminder dispatch cause. That lifecycle scheduling work remains separate. `cas-ad3d` / eager-pelican-23 owns consumed-injection queue accounting; ownership was coordinated and this delivery changes none of its runtime files.

| Acceptance | Verdict |
| --- | --- |
| Existing task-completion reminder dispatch/expiry fix retained in 3.84/3.86 | VERIFIED: version ancestry, prior RED/GREEN artifacts, current source tracing; 22/22 fresh named tests above |
| Visible installed-versus-supervisor MCP version warning | VERIFIED: startup/status/install wiring and existing live-epoch warning tests |
| Exact original 3.66 supervisor latency/model action | NOT EXERCISED: originating trace unavailable; no wall-clock production latency guarantee inferred from fixture transport |
| Browser/terminal rendering journey | NOT A ROUTE: documentation-only delivery |
| GH1406 result comment | VERIFIED: [posted evidence and limits](https://github.com/Richards-LLC/cassy/issues/1406#issuecomment-6470675073), authenticated `gh issue comment` exit 0 |

Design: `DESIGN.md@b43940e43`; plain Markdown evidence follows the project's factual voice. No product behavior, schema, gate, hook, configuration or builtin guidance changed.
