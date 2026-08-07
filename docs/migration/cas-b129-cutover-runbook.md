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

**G9 — Structural asserts will pass.** The dry run checks them, so G2 covers
this, but know what they are (`preconditions.rs:68-107`, spec §11): populated
`code_memory_links` / `entities` / `entity_mentions` / `relationships`;
compressed rows or any `raw_content`; a legacy MarkdownStore directory
(`<root>/entries` or `<root>/archive`). Each aborts rather than silently
dropping data with no destination.

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

### Step 1 — dry run against the live stores (read-only)

```
cas memory-migrate
```

Sources are opened `SQLITE_OPEN_READ_ONLY`; nothing is written to the knowledge
store. Expected tail: `DRY RUN — nothing was written to the knowledge store.`

Record the audit table and the quarantine list in the task notes. Gate on G2/G3.

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

**Expected shape, from the M3 rehearsal on copies of both live databases**
(1698 legacy rows: project 1248 + global 450). The live project DB has grown
since, so treat these as *shape*, not as exact expected values — the invariant is
the balance, not the constants:

```
migrate-to-page 160 | carry-verbatim 21 | stay-entry 439 | deliberately-left 1078 (R6 84) | merged-into 0
by rule: R1 994 (fixtures) · R3 21 · R4 1 · R5 11 · R6 84 · R7 156 · R8 4 · R9 427
```

### V2 — Pages written where they belong

Rows land in the knowledge store of their **own** CAS root — project rows into
the project root, global rows into `~/.cas`:

```
sqlite3 "file:$PWD/.cas/cas.db?mode=ro"  "select count(*), sum(locked) from knowledge_pages"
sqlite3 "file:$HOME/.cas/cas.db?mode=ro" "select count(*), sum(locked) from knowledge_pages"
```

Rehearsal produced 126 project + 55 global = 181 pages, of which **21 locked** —
exactly the carry-verbatim count. Locked pages are the pins and preferences whose
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

- **The replay covers the PROJECT store only.** `cas retrieval-parity` resolves
  its global side via `config::global_cas_dir()` = `~/.config/cas`
  (`cli/retrieval_parity.rs:70`, `config/access/global.rs:3-5`), which has no
  `cas.db` on this machine, and `ParityContext::with_global` drops a path without
  one (`retrieval_parity/mod.rs:161`). The migration's global side is `~/.cas`
  (`cli/memory_migrate.rs:114`). The committed baseline confirms it: `cas_dir` is
  the project `.cas` and the corpus is 1246 active entries — the project count.
  **A green replay says nothing about the 450-row global store or the 55 pages
  written into it.** Hence V6.
- Entries added since the baseline was captured (1246 → 1249 and counting) appear
  as `new_hits`, which are reported as upgrades and never fail.

### V6 — Global store, verified by hand

Because V5 is blind there, check the global side explicitly:

```
sqlite3 "file:$HOME/.cas/cas.db?mode=ro" \
  "select (select count(*) from entries) entries,
          (select count(*) from knowledge_pages) pages,
          (select count(*) from sync_queue where entity_type='entry') stranded"
```

Expect entries 450 (unchanged), pages 55 (rehearsal shape), stranded 0 if you
invalidated or drained. Confirm `sync-queue-invalidated.jsonl` has one line per
invalidated row and that every line carries a `cas_migration_db` stamp.

### V7 — Dual-read behaviour, observed rather than assumed

Start a fresh session and inspect the injected SessionStart context. Expected,
per §6: the memory blocks still appear (Pinned Memories, Helpful Memories) **and**
a `## 📚 Project Knowledge (N/M pages)` block appears. Record the actual `N/M`
from the header — the block is capped at 600 tokens and truncates (§6.2).

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
order of a dozen pages** fit. With 126 project pages the header will read
`(N/126 pages indexed)` with N far below 126.

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

Add the coverage limits from V5: the replay is project-only and blind to the
global store, and post-baseline entries appear as upgrades. And add §6.2: most
migrated pages are not even visible in SessionStart under the current 600-token
index budget.

Carry all of this to M6 / cas-7909 as the input to the read-path decision. The
honest post-cutover position is: *the legacy system still works exactly as it
did, and a parallel copy of 181 memories now exists as knowledge pages that
nothing but `mcp__cas__knowledge` and a truncated index block can reach.*

---

## 9. Quick reference

**Cutover, happy path**

```
cas memory-migrate                                     # 1. dry run — read the audit + quarantine
cas memory-migrate --apply --invalidate-sync-queue     # 2. apply (factory quiesced)
cas memory-migrate --reindex                           # 3. FTS rebuild + retrievability check
cas retrieval-parity replay                            # 4. parity (project-only — also do V6)
```

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
