---
metadata:
  managed_by: cas
---

# Worked example: v3.38.0

Replay the procedure on the actual release evidence, then look up current task
states before filing. This example records the cut and follow-up backlog at
cas-1997's start; later runs must honor tasks that have since closed.

## Bind and read

Resolve the configured release artifact base used by the cut (the train prints
`run directory:`; `CAS_RELEASE_ARTIFACTS_ROOT` may override the base). Paths
below are relative to that base; sibling task evidence is relative to the
configured factory artifact base and may still be in its legacy task directory.

- Final run: `v3.38.0-release-train-3380/`; published main SHA
  `a31c4bea9de390ad28cce15207969df6f6618052`. Read `run.env`, `gate.log`,
  `interventions.log`, `release-latency.receipt`, `host-update.json`,
  `worker-build-cache.log`, and `stage.*.done` receipts.
- Earlier failed run: `v3.38.0-release-train-3380-attempt1/`; read `gate.log`
  and `rows/20260929T225913Z-1485636/release-notes-shell-injection.log`.
- Console: `v3.38.0-cut-console.log`. Supplement with task notes and
  verification records for cas-cb01, cas-e075, cas-8e2f, cas-f40e and cas-37ba.
  The final green gate does not contain the earlier assembly failures.
- Supporting task artifacts: cas-e075 `cli-refusal.log`, cas-8e2f
  `red-final.log`, cas-f40e `count-reconciliation.md` and its observed-name
  records, and cas-37ba's committed test diff/evidence. Read QA ledgers and
  rule drafts/encode chores too; absence from this extract is an evidence gap.

## Eight observations, four existing follow-up tasks

Each row gives the observed failure and proposed environmental check. These
are all **automated checks**; do not turn mechanically decidable defects into
new prose standards. No category requires a finding just to fill the table.

| Case | Primary evidence | Mechanism and disposition at cas-1997's start |
| --- | --- | --- |
| Latency budget | Console reports 908s from tag push to publication against 600s; final interventions resume at post-publication twice | cas-cb01: retain a measured over-budget receipt/warning without blocking an already-published release; reject missing/invalid timestamps. |
| Zig-less cache refresh | Final `worker-build-cache.log` reports `Zig compiler not found`; `host-update.json` records refresh exited 101, despite gate's Zig row passing | cas-cb01: resolve/export executable Zig for the detached cache checkout and exercise resumed host-update in a fresh environment. Same owner, distinct acceptance from latency. |
| GitHub expression in shell | Earlier row log names `.github/workflows/ci.yml:937` and one unsafe run expression; final gate is green | cas-a006: move the existing workflow expression check before lane merge/compilation and into Scoped Validation; use env inputs. The historical edit is already fixed; add no duplicate scanner or reminder. |
| Disposable plain clone | cas-e075 `cli-refusal.log` rejects `/var/tmp/cas-release-gate/base`; its task records eight discovery/update failures | cas-e075 already fixed: early canonical scratch-root validation with `CAS_RELEASE_GATE_HOME_DIR`. Reuse cas-a006 for applicable cheap preflight coverage; do not reopen the historical bug. |
| Resume uses stale tip | cas-8e2f `red-final.log` says gate failure resume tested the stale integration tip; task notes capture advanced integration plus skipped assemble/prep/ledger | cas-8e2f already fixed: exact consumed integration tip/base invalidates the assembly suffix before publication, with edited metadata replay preserved. Record on epic; no new task. |
| Zero-test re-exec | cas-f40e evidence names three isolated child paths that needed qualified selectors and an executed-name/count guard | cas-0c55: guard every runner against zero execution, including intentional early-exit children. Historical paths already repaired by cas-f40e. |
| Dropped directory-main tests | cas-f40e reconciliation proves 33 additions minus 34 dropped hook tests; inventory omitted `tests/hooks_test/main.rs` | cas-f40e already fixed inventory108/10 and restored all34; reuse cas-a006 to wire this inventory check before merge without claiming net test count proves identity coverage. |
| Prose pins | cas-37ba records `test_supervisor_fix_round_recovery_guidance_present_and_mirrored` failing after two-context wording changed; its diff repairs the literal pin | cas-e229: consolidate the 804 phrase assertions in211 tests into a contract registry, preserving genuine cross-file contracts. Replace behavior claims with behavior checks, not new synonym pins. |

Expected output is the existing task list **cas-cb01, cas-a006, cas-0c55,
cas-e229**, with category, evidence and mechanisms attached. Expect zero new
tasks when these matches still cover the observations. With those tasks absent,
file the uncovered mechanisms; with them closed, first test the failure's time
against the fix before proposing a regression. Do not invent new tasks for
navigation, coding standards, AGENTS.md, tool economy, no-ops or information
access without evidence; record their reviewed/no-finding or unavailable status.

## Summary note shape

Append to the bound epic, rather than generating a report:

```text
Retro v3.38.0: final + failed-attempt logs and task evidence reviewed.
8 observations; new0; reused cas-cb01/cas-a006/cas-0c55/cas-e229.
Already repaired: disposable clone, stale resume, 3 child selectors,
34 missing directory-main tests, unsafe expression and the immediate pin.
Category: automated checks. Other categories: record actual review results.
QA ledgers, verification and rule drafts: name reviewed IDs or missing access;
this eight-case extract alone does not establish coverage of those sources.
```

Done when every observation has a cited source and a deduplicated disposition,
existing tasks carry new evidence, and the epic records any unreviewed sources.
