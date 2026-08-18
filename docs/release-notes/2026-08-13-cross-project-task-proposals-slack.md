# Slack draft — cross-project task proposals

**Status: POSTED.** The four bodies below were deduplicated against the live
channel, posted in the required order, and verified verbatim with exactly one
reply on each root.

Channel: `#cas-internal` (`C0B44GUKDK2`). Deploy label: **Live on production**.

=== MESSAGE 1 (user top-level) ===
Live on production — **User** — Work for another project can now be proposed and followed in Cassy without turning into a lost handoff.

=== MESSAGE 2 (user reply to MESSAGE 1) ===
Was → Now: Work for another project had to be copied or handed over manually, making its destination and outcome easy to lose track of. Now you can send one proposal to the intended project, see whether it is accepted or declined, and keep a dependent task honestly blocked until the work is finished.

=== MESSAGE 3 (dev top-level; new top-level, not a reply) ===
Live on production — **Dev** — Cassy now exposes creation, triage, and reconciliation for cross-project proposals while keeping each project's local task list truthful.

=== MESSAGE 4 (dev reply to MESSAGE 3) ===
Was → Now: Cassy could create and track tasks only in its current local project. Now proposal creation is idempotent and scoped to explicit origin and target projects; acceptance or rejection is auditable; source attribution is preserved; and dependency reconciliation deduplicates replayed pages while tracking reopen and resolution transitions.

## Posting sequence followed after confirmed main merge

1. Read `#cas-internal` and confirm these bodies have not already been posted.
2. Post MESSAGE 1 as a new top-level message and capture its timestamp.
3. Post MESSAGE 2 as the one reply to MESSAGE 1.
4. Post MESSAGE 3 as a new top-level message and capture its timestamp.
5. Post MESSAGE 4 as the one reply to MESSAGE 3.
6. Append `## POSTED` with UTC timestamps, channel, and four permalinks; then
   verify the live bodies match this file verbatim before committing the receipt.

## POSTED

Channel: `#cas-internal` (`C0B44GUKDK2`).

| Message | UTC timestamp | Permalink |
| --- | --- | --- |
| User top-level | 2026-08-13T17:46:37.879609000Z | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786643197879609 |
| User reply (Was → Now) | 2026-08-13T17:46:43.283019000Z | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786643203283019?thread_ts=1786643197.879609&cid=C0B44GUKDK2 |
| Dev top-level | 2026-08-13T17:46:47.870879000Z | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786643207870879 |
| Dev reply (Was → Now) | 2026-08-13T17:46:52.747129000Z | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786643212747129?thread_ts=1786643207.870879&cid=C0B44GUKDK2 |
