# cas-1037 cloud parity repair receipt

This receipt records the evidence-gated one-time repair needed before a fresh
machine pulls the `cas-src` project from CAS Cloud.

## Baseline

The 2026-08-09 fresh, project-scoped personal and team pulls were compared by
entity ID against the live local project store. The combined cloud set had
2,438 entries and 2,490 tasks; the local store had 2,449 entries and 2,219
tasks. Exact set differences were 11 local-only entries, 44 local-only tasks,
zero cloud-only entries, and 315 cloud-only tasks.

## Fixture deletion gate

Every cloud-only task had to pass every predicate below before deletion:

- absent from the local `tasks` table;
- project ID exactly `cas-src`;
- created from 2026-03-20 through 2026-06-07;
- status `open`, task type `task`, priority 2;
- empty description, acceptance criteria, and labels;
- title exactly one of `MCP Protocol Test Task`, `Context test task`,
  `Consolidated task test`, or `Test task for notification test`.

All 315 rows passed. Counts by title were 81, 79, 76, and 79 respectively.
The per-row evidence and delete result are in
`cas-1037-cloud-task-manifest.tsv`. No row failed verification, so the
operator-review residual is empty.

## Repair result

Pending live execution. This section is updated after backfill, purge, queue
drain, and an independent final parity pull.
