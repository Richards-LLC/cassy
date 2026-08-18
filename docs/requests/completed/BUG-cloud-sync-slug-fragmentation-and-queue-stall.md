---
from: ozer team (factory supervisor smooth-heron-80)
date: 2026-06-10
priority: P1
---

# Cloud sync: git-remote slug auto-derivation fragments a team off its canonical bucket; the push queue poison-head stalls all drainage; touching a task leaks duplicate queue items

Resolution: Already fixed by the cloud-sync reliability work merged in `34203e2` and released in `v2.21.0`: `be42057` fixed queue poison-head and duplicate NULL-team_id enqueue behavior, `28ac14c` added the `--rehome` guard and truthful push counts, and `b4d55d9`/`034a8a9` added bucket-ambiguity warning plus concrete team-show slug resolution.

A user reported that SOW-10 tasks created on one machine (Linux) were **invisible on a second machine** (Mac) after `cas cloud sync` on both. The tasks were never actually missing from the cloud — they were in the canonical `ozer` project bucket the whole time. The Mac simply **read a different bucket**, because Cassy 2.20 auto-derives the cloud project slug from the **git remote** instead of the established short slug. While diagnosing this, two further sync defects surfaced: the local push queue has a **poison-head FIFO stall** (pushes report success while draining nothing), and **touching a task enqueues duplicate items** that never fully clear. One diagnostic step also revealed that **pushing under a pinned slug silently re-homes ~17k entities between cloud buckets**.

## Affected version

`cas 2.20.0 (0780626-dirty 2026-06-10)`, cloud sync (`cas cloud push/pull/sync`), single user (`daniel@petrastella.io`) across two machines, project `Richards-LLC/ozer-health`.

## What I observed (evidence, same session)

### Symptom: cross-machine task invisibility
- Local store had **1952 tasks**; SOW-10 (epic `cas-e97c` + 11 children) created today 15:24–16:00 and present locally.
- `cas cloud team show` → **`Project slug: <not resolved>`**, `canonical_id: null` (nothing pinned).
- `git remote` = `git@github.com:Richards-LLC/ozer-health.git`.
- `cas cloud projects` showed **three** buckets for the same repo:
  - `ozer` — 2 contributors, **19285 memories** (the real shared history)
  - `ozer-health` — 0 contributors, 0 memories
  - `github.com/Richards-LLC/ozer-health` — 1 contributor, **666 memories** (what the git remote auto-derives)
- With no pin, the Mac auto-derives `github.com/Richards-LLC/ozer-health` and reads the near-empty 666-memory bucket → sees no SOW-10. `cas cloud sync` "succeeded" against the wrong bucket.

### Defect: queue never drains
- `cas cloud queue` → **1324 pending, 0 failed** (entry 970, task 260, rule 94), **oldest `2026-06-10T16:00:10`**.
- `cas cloud push` reported `[OK] Push complete` every run, but the queue only dropped by ~200 entries per run and then **froze at 1024** (task 260 and rule 94 *never moved across 4 consecutive pushes*). **The `oldest_item` timestamp never advanced** from `16:00:10` — the head item is never consumed, blocking everything behind it (FIFO).
- `cas cloud queue -v` showed **duplicate entries** in the queue (`upsert rule-005` twice, `rule-046` twice, …).

### Defect: push re-homes entities between buckets
- Pinning `canonical_id = "github.com/Richards-LLC/ozer-health"` and running `cas cloud push` moved the bucket counts from `ozer 19285 / git-bucket 666` to **`ozer 2404 / git-bucket 17549`** — ~17k entities silently re-tagged to the pinned slug. Total conserved (~19,950), so nothing was deleted, but a `push` (intuitively additive/idempotent) **mutated the project association of existing cloud entities**. Re-pinning `ozer` and pushing reversed it exactly (`ozer 19287 / git-bucket 666`).

### Defect: duplicate-enqueue leak + unreliable counts
- After `cas cloud queue --clear` (which did unblock the FIFO), touching one task via a progress note enqueued **2** task items; a single push drained **1**, leaving residue. Touching 10 tasks → queue showed 12 afterward.
- The push summary prints a **fixed boilerplate** (`Rules: 0 inserted, 47 updated`, `Events: 0 inserted, 10000 updated`, `File changes: …`) on runs that pushed only tasks, and **omits the `Tasks:` line entirely** on some runs that did move task items — so the printed insert/update counts can't be trusted. Only `cas cloud queue --json` deltas and (once unjammed) the `Tasks: N inserted/updated` line were reliable.

## Distinct defects

### A. Slug auto-derivation silently fragments a team off its canonical bucket
Deriving the cloud project slug from the git remote, with no pin, routes a machine to `github.com/<org>/<repo>` even when the team's data lives under a short canonical slug (`ozer`). Result: **silent cross-machine invisibility** — sync succeeds, against the wrong bucket. There is no warning that the resolved slug doesn't match where the data is, and `team show` renders `<not resolved>` rather than the slug it will actually use.

### B. Push queue poison-head FIFO stall
A single un-consumable item at the head of the queue (here, the `16:00:10` item) blocks all drainage. Pushes return `[OK] Push complete` while `oldest_item` never advances and pending count freezes — a **silent, success-reporting stall**. `failed` stays 0, so nothing signals the jam. Only `cas cloud queue --clear` cleared it.

### C. Duplicate-enqueue leak
A single task mutation enqueues ~2 queue items; push drains fewer than it enqueues, so residue accumulates indefinitely (compounding B — the residue is what stalls).

### D. `push` re-tags existing cloud entities to the pinned slug
`cas cloud push` is not purely additive: under a changed `canonical_id` it re-homes existing cloud entities into the pinned bucket. This is powerful but surprising and undocumented; a mis-pin can move a team's entire history into a stray bucket in one command (reversible only if you notice and re-pin).

## Impact

- **Silent data invisibility across a user's own machines** (the reported symptom) and, for multi-contributor projects, between teammates — with no error and a "successful" sync.
- **Sync can wedge indefinitely** (B/C) while reporting success — a user believes they're synced when nothing is moving.
- **A one-line slug change can re-home ~17k entities** (D); easy to trigger while troubleshooting, scary even though reversible.

## Suggested fixes

1. **Pin-by-default / warn-on-ambiguity.** On first cloud op in a repo, if multiple buckets resolve for the same remote (e.g. `ozer` vs `github.com/<org>/<repo>`), **prompt or warn** instead of silently picking the git-remote form. Write a `[project] canonical_id` on first resolve so it's stable and git-shareable.
2. **Make `team show` print the slug that will actually be used**, never `<not resolved>` for an active sync target.
3. **Don't let one bad item wedge the queue.** Process the queue resiliently (skip/park the failing item, advance the head, surface it under `failed` with a reason) rather than head-of-line blocking with `failed: 0`.
4. **Fix duplicate-enqueue** so one mutation enqueues one item, and ensure a push drains everything it can each run (the `200`-batch cap shouldn't silently leave a permanent residue).
5. **Make `push` insert/update counts truthful** (drop the fixed boilerplate; always print the real per-type counts) so operators can verify what landed.
6. **Treat slug re-homing (D) as an explicit, confirmed operation** — a `push` that would change the project association of existing cloud entities should warn / require a flag, not happen as a side effect of a normal push.

## Repro sketch

1. In a repo whose git remote is `github.com/<org>/<repo>` but whose Cassy history lives under a short slug (`<repo-short>`), leave `canonical_id` unset.
2. Create tasks on machine A; `cas cloud sync`. On machine B, `cas cloud sync` and list tasks → **they're missing** (B resolved the git-remote bucket).
3. On machine A, `cas cloud queue` → pending > 0 with an old `oldest_item`; run `cas cloud push` several times → `[OK]` each time, but `oldest_item` and the per-type counts don't move (poison-head stall).
4. `cas cloud queue --clear`, touch a task, `cas cloud queue --json` → enqueued count is ~2× the tasks touched.
5. `cas cloud project set <git-remote-slug>` + `cas cloud push`, then `cas cloud projects` → bucket memory counts shift by thousands (re-homing).

## Acceptance criteria

1. A machine with no pin either (a) resolves to the same bucket as the team's existing data, or (b) **warns** that the resolved slug has no/low data while another bucket for the same remote is populated — never silently syncs an empty bucket as success.
2. `cas cloud team show` reports the concrete slug used for sync (no `<not resolved>` on an active target).
3. A single un-pushable queue item cannot freeze the whole queue; it is parked/surfaced as `failed` with a reason and the rest drains.
4. Touching N tasks enqueues N items (not ~2N), and repeated `cas cloud push` drains the queue to 0 with no permanent residue.
5. `cas cloud push` prints accurate per-type insert/update counts on every run.
6. A `push` never changes the project association of existing cloud entities without an explicit, confirmed flag.
