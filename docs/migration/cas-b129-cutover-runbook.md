# cas-b129 M5 — Cutover and rollback runbook

Task: **cas-edee** (Phase 5 of epic **cas-b129**). Operational. This document is
what an operator follows to migrate this machine's legacy memory store into the
knowledge system, verify the result, and undo it.

Inputs, all merged on the epic branch:
[M1 inventory](./cas-b129-legacy-memory-inventory.md) ·
[M2 mapping spec](./cas-b129-mapping-spec.md) (normative) ·
M3 tool `cas memory-migrate` (cas-f4c1) ·
M4 parity harness `cas retrieval-parity` (cas-90fd,
[README](../../fixtures/retrieval-parity/README.md)).

Every claim below cites the code it comes from. Where a number came from a
rehearsal rather than the code, it says so, and says which copy it was measured
on.

---

## 0. What this cutover is — and what it is not

**It is additive.** The migration does not delete or modify a single row in
`entries`. Rows are *read*, routed through the §4 decision procedure, and — for
the two dispositions that produce output — written as new `knowledge_pages` rows
plus markdown bodies. `stay-entry` and `deliberately-leave` rows simply remain
where they are. This is why the rollback can be surgical (§7) and why the parity
replay is expected to pass (§8).

**It has exactly one destructive act on the source**, and it is opt-in: with
`--invalidate-sync-queue`, the stranded `sync_queue` rows whose `entity_type =
'entry'` are recorded in full and then deleted (spec §5.3). On this machine that
is **11 rows, all in the GLOBAL database** (verified read-only:
`~/.cas/cas.db` has 11, the project DB has 0). Their payloads are ledgered
*before* the delete, and the rollback restores them (§7).

**What it does NOT do — state this plainly to anyone reading a green result:**

| Not done by this cutover | Where it lives instead |
|---|---|
| Flip retrieval reads from `entries` to knowledge pages | Unbuilt. §6. Decision moves to M6 / cas-7909 |
| Delete the 994 test-fixture rows | cas-78c8 / GH #156 |
| Improve retrieval quality | Nothing on this epic measures it. §8 |
| Touch the Tantivy index | Deliberately never touched. §5 step 4 |
| Migrate the other CAS roots on this machine | Only the project root and `~/.cas` |

---

## 1. The commands involved

| Command | Effect | Default |
|---|---|---|
| `cas memory-migrate` | Route + audit + print quarantine | **Report only** |
| `cas memory-migrate --apply` | Also write pages | Writes |
| `cas memory-migrate --reindex` | Rebuild knowledge-page FTS from on-disk bodies, verify each page is retrievable | Writes FTS only |
| `cas memory-migrate --rollback` | Plan the undo from the ledger | **Report only** |
| `cas memory-migrate --rollback --apply` | Execute the undo | Deletes ledgered pages |
| `cas retrieval-parity capture` | Write a retrieval baseline | Writes one fixture file |
| `cas retrieval-parity replay` | Diff live retrieval against the baseline; **exit 1** on regression | Read-only |

Relevant flags: `--scope project|global|both` (default `both`),
`--project-root`, `--global-root`, `--ledger`, `--invalidate-sync-queue`,
`--page-size` (default 500). `--rollback` conflicts with `--reindex`
(`cli/memory_migrate.rs:67`).

The ledger defaults to `<resolved project root>/migration/cas-b129`
(`cli/memory_migrate.rs:16,151-156`) and holds:

| File | Contents |
|---|---|
| `applied.jsonl` | One record per page written — **this is what the rollback drives from** |
| `audit.json` | The loss audit |
| `quarantine.jsonl` | The R6 contamination list |
| `sync-queue-invalidated.jsonl` | Full payloads of deleted `sync_queue` rows, stamped with `cas_migration_db` |
| `page-index.json` | The `--reindex` report |
| `legacy-defaults.json` | Recorded legacy defaults |

**The ledger is the rollback.** If it is lost, the surgical rollback cannot run
and you are left with the break-glass path (§7.4). Treat the ledger directory as
the most important artifact the cutover produces.

---

## 2. Preconditions — every one is a gate

Do not proceed past a failing gate. Several of these are enforced by the tool
and will abort on their own; they are listed anyway so a failure is recognised
rather than debugged.

**G1 — Authorisation.** A supervisor sign-off note must be present on cas-edee
*before* any real-store apply (AC2), plus operator clearance. The tool cannot
enforce this one.

**G2 — Dry run clean on the real data.** Run §4 step 1. Required: `unaccounted
0` and `balance: OK — zero loss`. The audit is the five-term identity the
supervisor ruled binding: `migrated + carried-verbatim + stay-entry +
deliberately-left + merged-into == total`. `merged-into` must read **0** — the
always-suffix `rel_path` scheme makes collisions impossible, so a non-zero value
means the scheme failed, not that a merge happened.

**G3 — Quarantine reviewed.** The dry run prints every R6-quarantined row with
id, title and matched token. Read the list before authorising. Quarantined rows
stay in `entries`; they are never deleted.

**G4 — M4 baseline present and matching this machine.** The baseline path is
derived from the hostname (`retrieval_parity/mod.rs:207-219`); this machine is
`soundwave`, so the file is
`fixtures/retrieval-parity/baseline-soundwave.json` (committed). A replay
against a baseline captured on another machine is meaningless.

**G5 — Rollback rehearsed with proof.** Done; see §7.3. Re-rehearse if the
migration code has changed since.

**G6 — Live roots are page-free and ledger-free.** Nothing should exist to be
confused with this migration's output:

```
sqlite3 "file:$PWD/.cas/cas.db?mode=ro" "select count(*) from knowledge_pages"   # expect 0
sqlite3 "file:$HOME/.cas/cas.db?mode=ro" "select count(*) from knowledge_pages"  # expect 0
ls .cas/migration/cas-b129 2>/dev/null   # expect: no such directory
```

If a ledger directory already exists, a rerun will treat its rows as "already
migrated" and skip them. That is correct for a resume and wrong for a fresh run.

**G7 — `sync_queue` decision made.** The apply **aborts** on stranded entry rows
unless `--invalidate-sync-queue` is passed. Two legitimate choices:
drain them first with `cas cloud sync`, or pass the flag and accept that the 11
GLOBAL rows are removed (recoverable from the ledger). There is no third option
where the apply proceeds and the queue is left alone.

**G8 — Writers quiesced.** `assert_stable_count` aborts the run if the source
row count moves between the two reads (`memory_migration/preconditions.rs:113-122`,
spec §3 Rule D0) — "a loss audit over a moving corpus proves nothing". The
project database is under continuous factory write: it was observed at 1245→1246
during M1, 1246 at the M4 capture, 1248 during the M3 rehearsal, and **1249**
when this runbook was written. Stop the factory (or at least ensure no worker is
storing memories) before the apply, or expect to retry.

> **The quiesce is not total, and `assert_stable_count` cannot see the part that
> leaks.** During the first live cutover, with every agent held, three project
> rows (`2026-07-21-1`, `2026-07-21-5`, `2026-07-30-1`) still changed between the
> pre-apply backup and the post-rollback comparison — in exactly two fields each,
> `updated_at` and `stability`, with `stability` moving *down* (0.6376→0.6323,
> 0.3933→0.3800). That is the background **stability-decay sweep**, not the
> migration and not the rollback: the migration opens its sources
> `SQLITE_OPEN_READ_ONLY` and the rollback only deletes pages and re-inserts
> `sync_queue` rows, so neither code path can produce it. `assert_stable_count`
> compares `COUNT(*)` only, so a decay sweep passes straight through the gate.
>
> Consequence for verification: **do not expect a whole-table digest of `entries`
> to match a backup taken minutes earlier**, and do not read a mismatch as damage
> until you have diffed it row by row. A digest built from
> `id|content|tier|type|helpful|created` — the fields the migration would touch if
> it touched anything — *is* stable, and is the digest to compare. Reserve the
> `updated_at`/`stability` columns for a row-level diff.

**G9 — Structural asserts will pass.** The dry run checks them, so G2 covers
this, but know what they are (`preconditions.rs:68-107`, spec §11): populated
`code_memory_links` / `entities` / `entity_mentions` / `relationships`;
compressed rows or any `raw_content`; a legacy MarkdownStore directory
(`<root>/entries` or `<root>/archive`). Each aborts rather than silently
dropping data with no destination.

**G10 — Backups taken, and taken in the right order.** See §2.1.

### 2.1 Backups — cost, ordering, and what they are actually for

**What a backup is for here, and what it is not.** The normal undo path (§7) is
the ledger-driven rollback; it does not consult a backup and cannot be helped by
one. A backup buys exactly one thing: the break-glass path (§7.4) when the
**ledger is lost**. Take it anyway — it is cheap relative to the alternative —
but do not let its existence become the reason the ledger is treated casually.

**Cost, measured on this machine:**

| Database | File size | Notes |
|---|---|---|
| Project `.cas/cas.db` | **431 MB** (451,756,032 bytes) | Also carries a live ~4 MB WAL |
| Global `~/.cas/cas.db` | **86 MB** (89,935,872 bytes) | WAL empty at rest |
| **Both** | **~517 MB per full backup** | |

Consequences that follow from those numbers:

- **Never stage a backup in `/tmp`.** `/tmp` is tmpfs — RAM — on this host. Half
  a gigabyte of backup plus the rehearsal copies from §3 is a way to wedge the
  machine during the one procedure whose purpose is safety. Use
  `/mnt/datacube/staging` (543 GB free at time of writing).
- **Never `cp` the database.** The project DB has a non-empty WAL right now, so a
  file copy can be torn. Back up the same way the rehearsal makes copies — with
  `VACUUM INTO` from a read-only connection, which produces a consistent,
  compacted snapshot. The output is often *smaller* than the source file because
  free pages are dropped; that is not a defect and not data loss.

```
B=/mnt/datacube/staging/cas-b129-backup
mkdir -p $B
sqlite3 "file:$PWD/.cas/cas.db?mode=ro"   "VACUUM INTO '$B/project-cas.db'"
sqlite3 "file:$HOME/.cas/cas.db?mode=ro"  "VACUUM INTO '$B/global-cas.db'"
sha256sum $B/*.db | tee $B/SHA256SUMS
```

**Ordering — this is the part that is easy to get wrong.** Take both backups
**before any mutating step, and specifically before the `sync_queue` decision of
G7**, not just before the apply. The reason:

- Backing up **first** captures the 11 stranded GLOBAL `sync_queue` rows while
  they still exist, so a break-glass restore recovers the queue state along with
  everything else.
- Backing up **after** `--invalidate-sync-queue` produces a backup in which those
  rows are already gone. A break-glass restore from it would silently drop them,
  and their only remaining copy would be `sync-queue-invalidated.jsonl` — which
  is the ledger, i.e. the very artifact the break-glass path exists to survive
  the loss of.
- Backing up after a `cas cloud sync` drain is less bad — the rows were delivered
  rather than deleted — but the ordering rule stays the same, because "was it
  drained or invalidated?" is not a question you want to be reconstructing under
  pressure.

So the correct sequence is: **backup → G7 decision (drain or accept
invalidation) → apply**. Record the backup path and the `SHA256SUMS` output in
the task notes at the same time as the dry-run audit, so the two artifacts are
timestamped against each other.

Delete the backups only after the cutover has been verified (§5) and accepted;
until then they are the last resort if both the pages and the ledger are lost.

---

## 3. Rehearse on copies first — and how not to hit the live store while doing it

**The trap:** `CAS_ROOT` beats the working directory in root detection
(`store/detect.rs:53`, GH #157). Under a factory worker `CAS_ROOT` points at the
live database, so `cd`-ing into a copy of a `.cas` tree does **not** retarget the
command. Rehearsals must pass explicit roots.

Stage the copies on **disk, not `/tmp`**. `/tmp` is tmpfs on this host (RAM), and
each database is ~90 MB — copying both plus their ledgers is a quarter-gigabyte
of memory, and CAS warns above a 1 GiB tmpfs threshold. The approved staging
location is `/mnt/datacube/staging`.

Make copies with `VACUUM INTO` from a read-only connection — never `cp` a live
WAL database:

```
R=/mnt/datacube/staging/m5rehearsal
mkdir -p $R/proj $R/glob
sqlite3 "file:$PWD/.cas/cas.db?mode=ro"   "VACUUM INTO '$R/proj/cas.db'"
sqlite3 "file:$HOME/.cas/cas.db?mode=ro"  "VACUUM INTO '$R/glob/cas.db'"
```

Then drive every invocation with explicit roots **and** `CAS_ROOT` pointed at a
copy, so no resolution path can reach the live tree:

```
CAS_ROOT=$R/proj cas memory-migrate \
  --project-root $R/proj \
  --global-root  $R/glob \
  --ledger       $R/ledger
```

Three safety properties are already built in and worth knowing:

1. `--project-root` **without** `--global-root` is a hard error, because the
   global side would otherwise default to `~/.cas` — the live database, which
   `--apply` would write pages into (`cli/memory_migrate.rs:103-110`).
2. The ledger defaults under the **resolved** project root, so a rehearsal
   cannot leave its ledger in the live tree where a later real run would read it
   as "already migrated" (`cli/memory_migrate.rs:148-156`).
3. The same root is never migrated twice under two labels — relevant only if the
   project root *is* `~/.cas` (`cli/memory_migrate.rs:117-125`).

The command prints a banner naming every database it is about to read and every
root it will write pages into, before any work starts
(`cli/memory_migrate.rs:168-185`). **Read the banner.** A mis-resolved root is a
mutation of live data, not a bad report.

After the rehearsal, confirm the live roots are still untouched (G6 checks) and
delete the copies.

---

## 4. Cutover

Run these in order, from the repo root, with the factory quiesced (G8).

### Step 0 — backups

Per §2.1, before anything mutating and before the G7 `sync_queue` decision.
`VACUUM INTO` both databases to `/mnt/datacube/staging`, record the sha256s.

### Step 1 — dry run against the live stores (read-only)

```
cas memory-migrate
```

Sources are opened `SQLITE_OPEN_READ_ONLY`; nothing is written to the knowledge
store. Expected tail: `DRY RUN — nothing was written to the knowledge store.`

Record the audit table and the quarantine list in the task notes. Gate on G2/G3.

### Step 1b — re-capture the M4 baseline, INSIDE the frozen window

```
cas retrieval-parity capture
```

Do this **after** the factory is quiesced (G8) and **immediately before** the
apply — not hours earlier. This is the only writing step before Step 2, and it
writes exactly one fixture file. Run 1 skipped it and V5 failed with 159
regressions caused entirely by four entries written after the old baseline; see
V5 and §8.1. Note the capture timestamp: §8.1's triage needs it.

### Step 2 — apply

```
cas memory-migrate --apply --invalidate-sync-queue
```

(Drop `--invalidate-sync-queue` if you drained the queue with `cas cloud sync`
first; keep it if you accepted the 11-row invalidation at G7.)

Expected tail: `applied N page(s); 0 already migrated by an earlier run`,
followed by the sync_queue line naming the ledger file, followed by the `NEXT:`
hint pointing at `--reindex`.

If the run is interrupted, **re-run the same command**. Resumption is keyed on
`cas_legacy_id` in the ledger, not on row order; a genuine SIGKILL mid-apply and
a truncated ledger were both proven to converge to the identical end state
(cas-f4c1 AC3). Re-running a *completed* apply is also safe: it reports
`applied 0; N already migrated`.

### Step 3 — reindex

```
cas memory-migrate --reindex
```

This rebuilds each page's FTS row from the **on-disk body** (the authoritative
copy, not the DB snippet) inside one transaction, then probes every page with a
real `MATCH` constrained to its own rowid, so "the writer ran" cannot pass for
"the page is retrievable". A missing body file or a written-but-unsearchable
page is a hard error. The report lands in `page-index.json`.

### Step 4 — what is deliberately NOT run

Do not rebuild the Tantivy index. It holds **no** knowledge-page documents:
`DocType::KnowledgePage` is only a label attached to hits produced by the
store-backed channel, and pages are reached through `KnowledgeStore::search` →
the SQLite contentless FTS5 table `knowledge_pages_fts`
(`knowledge_store.rs:1189-1215`). Rebuilding Tantivy would be a destructive
no-op *for pages* while invalidating the entries index that the surviving
`stay-entry` rows still depend on — and `SearchIndex::open` deletes and recreates
the index directory on a field-count mismatch (`search_index_impl.rs:78`).

---

## 5. Verification

### V1 — Loss audit balances

From the apply output (and `audit.json`): `unaccounted 0`, `balance: OK — zero
loss`, `merged-into 0`.

**Expected shape, from the run-2 rehearsal on fresh copies of both live databases
with the widened R6 predicate** (1700 legacy rows: project 1250 + global 450).
The live project DB is still growing, so treat these as *shape*, not as exact
expected values — the invariant is the balance, not the constants:

```
migrate-to-page 125 | carry-verbatim 21 | stay-entry 437 | deliberately-left 1117 (R6 123) | merged-into 0
by rule: R1 994 (fixtures) · R3 21 · R4 1 · R5 11 · R6 123 · R7 121 · R8 4 · R9 425
```

For comparison, the **run-1** shape (original five-token R6, the run that was
applied and then rolled back) was `migrate-to-page 160 | carry-verbatim 21 |
stay-entry 441 | deliberately-left 1078 (R6 84)`. The widening moved 39 rows from
page-bound to quarantined; see §8.2. If you see the run-1 numbers, **you are
running a binary built before the widening** — stop and rebuild.

### V2 — Pages written where they belong

Rows land in the knowledge store of their **own** CAS root — project rows into
the project root, global rows into `~/.cas`:

```
sqlite3 "file:$PWD/.cas/cas.db?mode=ro"  "select count(*), sum(locked) from knowledge_pages"
sqlite3 "file:$HOME/.cas/cas.db?mode=ro" "select count(*), sum(locked) from knowledge_pages"
```

The run-2 rehearsal (widened R6) produced **107 project + 39 global = 146 pages**,
of which **21 locked** — exactly the carry-verbatim count. (Run 1, before the
widening, produced 126 + 55 = 181.) Locked pages are the pins and preferences whose
bodies are byte-identical to the legacy `content`; distillation may never rewrite
them.

### V3 — `entries` untouched

```
sqlite3 "file:$PWD/.cas/cas.db?mode=ro"  "select count(*) from entries"   # unchanged from step 1
sqlite3 "file:$HOME/.cas/cas.db?mode=ro" "select count(*) from entries"   # 450
```

Any change here is a defect: no disposition deletes a row.

### V4 — Reindex report

`page-index.json`: pages reindexed == pages verified retrievable == page count,
`unindexable 0`.

### V5 — M4 retrieval parity replay

```
cas retrieval-parity replay
```

Expected: all cases PASS, exit 0. A regression exits 1 and names the missing
fingerprints. Hits match on **normalized-content fingerprint**, not entry id, so
re-keying cannot produce false regressions; gained hits and upward rank moves are
never regressions.

**Read the result correctly — see §8.** Two coverage limits apply, and both are
properties of the harness, not of the migration:

- **The replay now reaches the global store — check that it says so.** Until
  cas-96ae it did not: the global side resolved to `config::global_cas_dir()` =
  `~/.config/cas`, which has no `cas.db` on this machine, and
  `ParityContext::with_global` dropped that path *silently*, so every green
  replay covered the project store alone. It now resolves `~/.cas` (the
  migration's global side, `cli/memory_migrate.rs:114`), prints
  `global store: /home/pippenz/.cas` at the top of the run, and the
  `global-list-default` case records real global hits. Two things to verify
  before trusting a green:
  - the run printed the resolved global path, **not** a `WARNING:` — an
    unreachable global store is now reported as `unavailable`, never as a
    zero-hit pass; and
  - `global-list-default` PASSED with a non-zero hit count.

  Note that `session-merge` still cannot stand in for this: it lists project
  rows first and truncates at its limit, so with 1250+ project rows the global
  tail never reaches the baseline. V6 remains the hand-check for the pages
  written into the global store.
- 🔴 **The corpus MUST be frozen between capture and replay — re-capture the
  baseline INSIDE the quiesce window, immediately before the apply.** An earlier
  revision of this runbook said that entries added after the capture "appear as
  `new_hits`, which are reported as upgrades and never fail". That is true of the
  gained rows and **false of everything else**, and the first live cutover failed
  on exactly this: 23 cases, 16 passed, **159 regressions**, exit 1 — with the
  migration provably innocent. Only four entries had been written after the
  baseline, but the orderings are `created DESC` / recency, so four new rows take
  the head and shift **every** baseline hit down four positions. Against a rank
  tolerance of 3 that is a uniform "fell 4 positions" failure, and it evicts
  ranks 21–24 out of the fixed 25-row window, where the harness correctly reports
  them as `missing_hit`. New rows do not merely add hits; they **shift ranks and
  push tail hits off the end**. Three of the four drift rows were written by this
  epic's own workers.

  So: `cas retrieval-parity capture` goes inside the frozen window, after the
  quiesce gate G8 and before Step 2, and the replay diffs against *that* baseline
  — not against a baseline captured hours earlier. If you must replay against an
  older baseline, a regression is uninterpretable until you have re-run the
  three-way triage of §8.1.

### V6 — Global store, verified by hand

Because V5 is blind there, check the global side explicitly:

```
sqlite3 "file:$HOME/.cas/cas.db?mode=ro" \
  "select (select count(*) from entries) entries,
          (select count(*) from knowledge_pages) pages,
          (select count(*) from sync_queue where entity_type='entry') stranded"
```

Expect entries 450 (unchanged), pages 39 (run-2 rehearsal shape; run 1 gave 55), stranded 0 if you
invalidated or drained. Confirm `sync-queue-invalidated.jsonl` has one line per
invalidated row and that every line carries a `cas_migration_db` stamp.

### V7 — Dual-read behaviour, observed rather than assumed

Start a fresh session and inspect the injected SessionStart context. Expected,
per §6: the memory blocks still appear (Pinned Memories, Helpful Memories) **and**
a `## 📚 Project Knowledge (N/M pages)` block appears. Record the actual `N/M`
from the header — the block is capped at 600 tokens and truncates (§6.2).

🔴 **V7 is the gate run 1 failed, and it is not satisfied by the block merely
rendering.** Read the page titles the block actually contains and confirm every
one of them belongs to this project. In run 1 the header read
`(3/126 pages indexed)` — far more aggressive truncation than §6.2's "about a
dozen" estimate — and two of those three pages were another project's HUD-1 and
settlement statement (§8.2). Because the index sorts by `(page_type, title, id)`,
the pages that win the budget are the alphabetically-first `context` pages, so
**contamination surfaces here first and most visibly.** Also record the payload
size against the 9216-byte budget: run 1 grew it 13,632 → 14,700 B.

If any injected title is not cas-src's, **stop and report** — that is a
quarantine-predicate gap, not something to accept in the window.

---

## 6. Dual-read during the transition, and the single-read flip

### 6.1 What actually reads what

| Surface | Reads `entries` | Reads `knowledge_pages` |
|---|---|---|
| SessionStart — Pinned Memories (`build_start.rs:287-289`, `store.list_pinned()`) | ✅ | — |
| SessionStart — Helpful Memories (`build_start.rs:146`, `merge_entries` → `store.list()`) | ✅ | — |
| SessionStart — Project Knowledge index (`build_start.rs:762-764`, `ks.list_pages()`) | — | ✅ |
| `mcp__cas__search` (`mcp/tools/core/search.rs:190`, `SearchIndex::search_unified`) | ✅ | ❌ — the `doc_type` map has no knowledge case (`:139-149`) |
| `HybridSearch` knowledge channel | ✅ | ❌ off by default (`hybrid_search/hybrid.rs:115`) |
| `mcp__cas__knowledge` | — | ✅ |

So "dual-read" is real but **asymmetric and narrow**: it exists *only* in the
SessionStart injection. Search — the surface an agent actually reaches for — is
entries-only today and stays entries-only after the cutover. Pages are reachable
through `mcp__cas__knowledge` and the SessionStart index block, and nowhere else.

Two consequences to expect and not misdiagnose:

- **A migrated memory can appear twice in SessionStart** — once as an entry line,
  once as a knowledge index line. Nothing gates one section on the other; the only
  conditional is that the knowledge block is suppressed when there are no pages
  (`build_start.rs:46-48`). There is no cross-store dedup.
- **If the knowledge channel is ever switched on**, a migrated memory returns
  **twice** with independent ranks: the hybrid merge is a union keyed on the id
  string only (`hybrid.rs:626-651`) with `doc_type` assigned by id membership
  (`:660-664`). There is no content/body/hash dedup and no `cas_legacy_id`
  back-reference anywhere in the search path. The only fingerprint dedup that
  exists is inside the M4 parity harness, not in the runtime.

### 6.2 The knowledge index block truncates

`render_knowledge_index` sorts pages by `page_type`, then `title`, then `id`, and
emits lines until the 600-token budget is exhausted, then stops
(`build_start.rs:42-104`, `KNOWLEDGE_SECTION_TOKEN_BUDGET = 600`,
`KNOWLEDGE_SNIPPET_CHARS = 120`, `estimate_tokens` = `len/4`). A typical line —
id, type, title and a 120-char snippet — costs roughly 50 tokens, so **on the
order of a dozen pages** fit. With 107 project pages the header will read
`(N/107 pages indexed)` with N far below 107. **Measured in run 1 the real
number was 3 of 126** — the estimate below is optimistic by a factor of four.

This is not a migration defect; it is the existing index-inject budget meeting a
much larger page set for the first time. But it means **most migrated pages are
not visible in SessionStart** and are reachable only by an explicit
`mcp__cas__knowledge` call. Record the observed `N/M` at V7 and carry it to M6.

### 6.3 The single-read flip does not exist

There is **no** config key, env var, feature flag or code path anywhere that
switches reads from both stores to knowledge-only. A repo-wide search for
`knowledge_only`, `single_read`, `dual_read`, `prefer_knowledge`,
`migration_complete` across `.rs` and `.toml` returns **zero hits** (re-verified
while writing this document). `MemoryConfig` holds exactly one field,
`session_learn_auto` (`config/settings.rs:910-919`) — a write-path Stop-hook
flag. There is no `[knowledge]` config section and no read-path key at all.

AC4 is therefore satisfied by naming the flip as unbuilt, not by documenting a
condition that isn't there. Building it — **out of scope for M5, carried to M6 /
cas-7909** — would require at minimum:

1. A read-path config key (there is no section to put it in today).
2. Wiring the knowledge store into `HybridSearch` in production:
   `set_knowledge_store` / `set_knowledge_store_from_path` currently have **zero
   callers outside `hybrid.rs`** (the only call site is a test, `hybrid.rs:1002`),
   and `enable_knowledge: true` appears only in tests (`:1084`, `:1147`).
3. Cross-store dedup keyed on something other than the id string, or the double
   results described in §6.1 become the default experience.
4. A decision for `mcp__cas__search`, which does not use `HybridSearch` at all —
   it calls `SearchIndex::search_unified`, whose `doc_type` map has no knowledge
   case. Pages would need either a Tantivy document type or a separate channel.
5. Suppression logic for the SessionStart entries blocks, which are currently
   unconditional.
6. A retrieval-quality measurement, since nothing on this epic provides one (§8).

---

## 7. Rollback

### 7.1 Why it is surgical rather than a database restore

`cas.db` is **not** a memory database. It also holds tasks (1522 at the time of
writing), task leases, agents, sessions, verification records, worker delivery
transactions, the prompt and supervisor queues, and the worktree registry — all
under continuous write by a live factory. Restoring a pre-migration `cas.db`
would not roll back the migration; it would roll back the **entire system** to
the backup instant, silently discarding every task closure, verification and
message recorded since. That is a larger and far less reversible loss than the
one being undone.

So the rollback is driven from the ledger: for each page this migration actually
created, remove that page. Anything the migration never touched is not in the
ledger and cannot be affected. **Approved by the supervisor as the primary path**
(decision note on cas-edee, 2026-08-07).

### 7.2 Running it

```
cas memory-migrate --rollback              # plan only — deletes nothing
cas memory-migrate --rollback --apply      # execute
```

Report-only by default, exactly like the migration. The plan and the execution
print the same table: ledgered pages, pages removed, body files removed, already
absent, diverged (NOT touched), sync_queue rows restored.

Semantics that matter:

- **It refuses rather than guesses.** A ledger record is acted on only if the
  page still at that id has the `rel_path` the ledger recorded. If it does not,
  the page was replaced after the migration; it is reported as **diverged** and
  left alone (`rollback.rs:172-186`). Divergence does **not** count as success:
  `is_clean()` is false and the command exits non-zero. Silently deleting a page
  that no longer matches would make the rollback a second data-loss event.
- **It is idempotent.** A second rollback reports every page as `already absent`
  and stays clean. `sync_queue` restoration is `INSERT OR IGNORE` on the recorded
  columns, so running it twice — or after you already drained the queue with
  `cas cloud sync` — cannot duplicate a row (`rollback.rs:192-197`).
- **It never re-reads the legacy corpus**, so a source database that has moved on
  since the apply cannot change what gets removed
  (`cli/memory_migrate.rs:187-191`).
- **Bodies and FTS go with the page.** `delete_page` removes the
  `knowledge_pages` row and its `knowledge_pages_fts` row in one transaction, then
  removes the body file (`knowledge_store.rs:1158-1187`).
- **An unstamped `sync_queue` payload is a hard error**, never a guessed
  destination (`rollback.rs:276-288`) — see the warning in 7.3.

If it exits non-zero, stop. The message names how many diverged and how many of
the ledgered pages resolved. Nothing further is attempted; resolve by hand.

### 7.3 Proof — the rehearsal, and the bug it caught

Exercised end to end on `VACUUM INTO` copies of **both** real databases (commit
`22146e97`). No live store was touched at any point.

```
PRE       proj pages=0   entries=1249 syncq=0  | glob pages=0  entries=450 syncq=73 sha=ba8d5e93244590b72c3c | bodies=0
APPLY     proj pages=126              syncq=0  | glob pages=55             syncq=62 sha=ff2ca3fd8ffb56a9a5cb | bodies=181
ROLLBACK  181 ledgered / 181 removed / 181 bodies removed / 0 already-absent / 0 diverged / 11 sync_queue restored / clean
POST      IDENTICAL to PRE on every axis, including the sha256 of the FULL sync_queue contents
```

The dry run changed nothing (pages still 126/55 after the plan). A second
rollback reported 181 already-absent and stayed clean.

> **⚠️ Warning example — a rollback can be a second corruption.** The *first*
> rehearsal restored the 11 **GLOBAL** `sync_queue` rows into the **PROJECT**
> database (project 0→11, global 73→62). Cause: the invalidation ledger recorded
> the payloads but not which database they came from, so the restore wrote to the
> first root that had a `sync_queue` table. The fix: invalidation now stamps
> `cas_migration_db` on every payload, restore routes by that stamp, and an
> unstamped payload is a hard error rather than a guessed destination. This is
> why the rollback had to be *exercised* before it could be written down —
> reading the code would not have found it.

Axes verified: page counts, entry counts, `sync_queue` row count **and** the
sha256 of its full contents, body files on disk, and idempotence. The FTS table
is removed transactionally with the page (7.2) but was not asserted
independently; if you ever run a real rollback, check
`select count(*) from knowledge_pages_fts` alongside the page count.

### 7.4 Break-glass only — full database restore

Restoring a `cas.db` backup is **not** part of the normal rollback path. Use it
only when all of the following hold, and say so in writing on the task:

1. The factory is stopped and no other agent or session is writing.
2. You are the single user of the machine at that moment.
3. You accept that **every task closure, verification, message, lease and
   delivery receipt recorded since the backup is discarded** — not just the
   migration.
4. The surgical rollback has already failed or is impossible (ledger lost).

The backup it restores from is the one taken at §2.1 / step 0 —
`/mnt/datacube/staging/cas-b129-backup/{project,global}-cas.db`. Verify it
against the recorded `SHA256SUMS` before restoring; a backup nobody checked is a
belief, not a backup. If it was taken *after* the `sync_queue` decision, the 11
stranded rows are not in it (§2.1 ordering) — which is precisely the failure this
path cannot rescue you from, since the ledger holding their payloads is by
assumption already lost.

If you do it, back up the *current* `cas.db` first, so the choice remains
reversible.

---

## 8. How to read a green result

A PASS at V5 is close to guaranteed **by construction**, and it must not be
over-read. M3 does not delete or modify a single `entries` row, and every M4
channel reads `entries`. So:

> A green replay proves **the migration did not disturb the legacy read paths**.
> It does **not** prove that knowledge retrieval is as good as, or equivalent to,
> memory retrieval. **Nothing on this epic measures that.**

Add the coverage limits from V5: the global store is measured only by
`global-list-default` (`session_merge` truncates it away), and post-baseline
entries appear as upgrades. And add §6.2: most
migrated pages are not even visible in SessionStart under the current 600-token
index budget.

Carry all of this to M6 / cas-7909 as the input to the read-path decision. The
honest post-cutover position is: *the legacy system still works exactly as it
did, and a parallel copy of 146 memories now exists as knowledge pages that
nothing but `mcp__cas__knowledge` and a truncated index block can reach.*

### 8.1 Triaging a RED parity replay — prove the cause, do not argue it

A V5 failure is not self-explaining. Before reporting it as migration damage,
run all three of these; run 1 failed V5 with **159 regressions** and all three
proved the migration innocent:

1. **Are the "missing" entries still in the database?** Query each reported
   fingerprint's entry back by id from the live store. If it answers, the row was
   never lost and the failure is about *ranking*, not existence.
2. **Did the corpus move after the baseline was captured?**
   `select count(*) from entries where created > '<baseline capture time>'`. Any
   non-zero answer can shift every hit down that many positions (see V5). Compare
   the count to the reported rank drops — run 1 had exactly 4 new rows and a
   uniform "fell 4 positions".
3. **DECISIVE — is the `entries` table byte-identical to the pre-apply backup?**
   Digest `id|content|tier|type|helpful|created` over all rows, ordered, in both
   the backup and the live DB, for **both** stores. Every parity channel reads
   `entries`, so if the digests match, the migration changed nothing any channel
   can see and cannot be the cause. Exclude `updated_at`/`stability` — the decay
   sweep moves those without the migration's involvement (G8).

If 1–3 all hold, the correct disposition is *baseline drift, accepted in writing*
— which is what AC3 permits — **not** a rollback.

### 8.2 Contamination — why R6 was widened after run 1

Run 1 applied cleanly and passed V1–V4 and V6, then failed at the surface nobody
had measured: the SessionStart knowledge index. Sorted by `(page_type, title, id)`,
the three pages that won the 600-token budget were the alphabetically-first
`context` pages — and **positions 1 and 2 were another project's real-estate
closing documents**, "105 Leake Ave #44 — Original Purchase Settlement" and
"202 Moultrie Park — Original Purchase HUD-1 — CSL Loan $1.33M". Every cas-src
session would have opened with two HUD-1 settlement statements injected.

Nothing was lost and nothing leaked outside the machine — but 29 of the 181 pages
were another project's records, and the migration had *promoted* them from an
inert archive-tier row that no read path surfaced to the top of an always-on
context surface. That is a retrieval-quality regression, and precisely the class
of thing R6 exists to prevent. The operator's decision was to roll back rather
than accept it.

**The fix, and the rule it teaches.** R6's token list was widened with nine
property/loan/settlement proper nouns — `Roark`, `Realty`, `JRPW`, `Renovo`,
`Leake`, `Moultrie`, `Radnor`, `Old Hickory`, `HUD-1` — raising the quarantine
from 84 to 123 rows and the page count from 181 down to 146. The widening is
**monotone**: 39 rows newly quarantined, none un-quarantined.

The rule worth carrying: **quarantine tokens must be proper nouns, never generic
English.** Measured against the real 1700-row corpus, three candidates that
looked obvious were rejected because they match genuine cas-src prose —
`Property` matches `Object.getOwnPropertyNames(...)`, `Lease` matches the
`LeaseNotFound` error type, and `Richards` matches the `Richards-LLC` GitHub org
and Vercel team that appear throughout live infrastructure memories (its marginal
contribution was 10 rows, all of them legitimate). A further sixteen candidates
(`Escrow`, `Mortgage`, `Loan`, `Settlement Statement`, `Warranty Deed`, `1098`,
`K-1`, `CSL`, `Ingram`, `Rearden`, …) were measured and add **zero** rows beyond
the proper nouns, so none was added. Both directions are locked by unit tests in
`memory_migration/routing.rs`.

Three of the 39 are genuine cas-src memories caught because they *mention* the
other project's paths in their bodies (`Local CAS project layout`, `CAS Cloud
auth env vars`, and Ben Richards' Slack profile). Quarantine is
stay-entry-in-place, so they are not lost — only not promoted. That is the
accepted cost of matching content as well as title.

**Before any re-apply, re-read the printed quarantine list.** Per the E1 ruling
the dry run prints every quarantined row with id, title and matched token, and a
supervisor must review both the token list and that list before the apply is
authorized.

---

## 9. Quick reference

**Cutover, happy path**

```
# 0. backups FIRST — before the sync_queue decision, not just before the apply (§2.1)
B=/mnt/datacube/staging/cas-b129-backup; mkdir -p $B
sqlite3 "file:$PWD/.cas/cas.db?mode=ro"  "VACUUM INTO '$B/project-cas.db'"   # 431MB source
sqlite3 "file:$HOME/.cas/cas.db?mode=ro" "VACUUM INTO '$B/global-cas.db'"    #  86MB source
sha256sum $B/*.db | tee $B/SHA256SUMS

cas memory-migrate                                     # 1. dry run — read the audit + quarantine
cas retrieval-parity capture                           # 1b. re-baseline INSIDE the frozen window
cas memory-migrate --apply --invalidate-sync-queue     # 2. apply (factory quiesced)
cas memory-migrate --reindex                           # 3. FTS rebuild + retrievability check
cas retrieval-parity replay                            # 4. parity (confirm the global store line — also do V6)
```

**Two things that are easy to get wrong**

- The released `cas` binary has **no `memory-migrate` subcommand** — it exists
  only on the epic branch. Build from the worktree (`cargo build -p cas --bin
  cas`) and invoke `./target/debug/cas`, or every step above fails with "unknown
  subcommand".
- Expected run-2 shape is **146 pages / R6 123**. If you see **181 / 84** you are
  running a pre-widening binary (§8.2).

**Undo**

```
cas memory-migrate --rollback                          # plan
cas memory-migrate --rollback --apply                  # execute
```

**Abort conditions** — stop and escalate on any of: `unaccounted != 0`,
`merged-into != 0`, `balance: FAILED`, a source row count that moved during
extraction, a populated graph table, a compressed/`raw_content` row, a
MarkdownStore directory, `entries` counts changing across the apply, a
non-zero exit from `--reindex`, `diverged != 0` at rollback, or a `sync_queue`
payload without a `cas_migration_db` stamp.
