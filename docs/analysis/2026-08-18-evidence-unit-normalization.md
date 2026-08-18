# Evidence-unit normalization and continuous read-only ingestion (M1)

**Task:** cas-b78b · **Epic:** cas-0cda (Operational intelligence v2) · **Date:** 2026-08-18

M1 is the normalization layer between the corpora that already exist and the
query layer that M2 (cas-2556) and M3 (cas-c214) build on. It does not create a
new corpus. It reads the same sources the existing corpora were built from —
the coordination database, daemon logs, and Claude/Codex/Grok transcripts — and
turns them into **typed evidence units** that carry provenance and join across
session, task, worker, commit, file, symbol, and deployed-binary epoch.

Implementation: `docs/analysis/scripts/evidence_units.py`
Claim registry: `docs/analysis/evidence_claims.json`
Tests: `docs/analysis/scripts/test_evidence_units.py`

## What it inherits rather than reinvents

| Inherited from | What is reused |
|---|---|
| `docs/analysis/scripts/historical_vector_index.py` (cas-c505) | The dedupe-before-embed pipeline: strip boilerplate, redact secrets/emails, structurally normalize, hash. cas-c505 measured 92.17% structural reduction with it. |
| `docs/analysis/scripts/mine_failure_modes.py` (cas-9d92) | The window/candidate/adjudication shape. Lexical matching only ever nominates a candidate; an authority adjudicates it. |
| `cas-cli/src/history/epochs.rs` + `history_epochs` (cas-8d2a / M8) | The deployed-binary epoch model, including the rule that a window with more than one live version is `mixed:` and is never post-fix evidence. |

The frozen cas-c505 index is a read-only input. The write guard refuses any path
under `artifacts/cas-c505/` outright, so the continuous lane cannot append to
that cutoff artifact even by accident.

## The four hard properties

### Read-only, and never into memory or knowledge

Every source handle is opened with `mode=ro` **and** pinned with
`PRAGMA query_only = ON`, so a write attempt fails at the handle rather than
relying on the caller's discipline. Every write target passes through
`assert_writable()`, which admits only paths under the declared namespace root.
The `entries` (memories) and `knowledge_pages` (knowledge corpus) tables are
never read into the namespace and never written anywhere; a test greps the
implementation for `INSERT/UPDATE/DELETE` against those table names so the
property cannot rot silently.

The coordination database is snapshotted (db + WAL + SHM copied into the
namespace, then `PRAGMA integrity_check`) and queried from the copy. That is
what makes ingestion safe to run while the daemon is writing.

### Incremental and resumable

| Source kind | Cursor | Restart rule |
|---|---|---|
| `tasks`, `events`, `prompt_queue`, `supervisor_queue` | monotonic rowid | resume after the highest ingested rowid |
| daemon logs, Claude/Codex/Grok transcripts | byte offset + line number | resume at the byte offset |

Rotation and truncation are detected by comparing the stored inode and size
against the current stat; either mismatch restarts that source at offset zero
rather than seeking past the end of a new file. A second run over unchanged
sources reads zero candidates.

Each run is budgeted (`--max-rows` per table, `--max-bytes` per file) so a
continuous schedule never turns into an unbounded sweep, and the watermarks make
the next run pick up exactly where the budget ran out.

### Scoped and retained

Every observation carries a privacy scope:

| Scope | Assigned to | Meaning |
|---|---|---|
| `host` | daemon logs, transcripts outside the project | never leaves this machine |
| `project` | coordination rows, transcripts inside the project worktrees | shareable within the project |
| `team` | rows carrying a `team_id` | shareable with the linked team |

Deduplication merges text observed in more than one place, so a unit keeps the
**most restrictive** scope of any of its observations — a line seen in both a
daemon log and a task note is `host`. `--scopes` restricts a run to a subset.

`retention` deletes provenance past a per-scope age and then removes units left
with no provenance, writing one hashed receipt per scope recording the policy,
the cutoff, the counts deleted, and the oldest row retained. Two deliberate
exceptions: provenance with no usable timestamp is retained rather than deleted
on an unknown age, and correction units are never deleted — losing the record
that retired a claim is the one deletion that would make retrieval less safe.

### Correction-aware

cas-c505 measured a retrieval **safety failure**: for a claim cas-9d92 had
already withdrawn, neither lexical nor vector ranking surfaced the authoritative
correction, because repeated historical prose about the claim outranked it. Only
inspecting current source caught it.

M1 fixes that structurally rather than by ranking tricks:

1. `evidence_claims.json` registers a claim, its lexical patterns, and the
   authorities that corrected it (with `file:line` pointers into current source).
2. Ingestion tags every matching unit with the claim key. A unit that matches
   the claim **and** carries a correction marker is recorded as the correction,
   not as another assertion of the claim — the adjudication step inherited from
   `mine_failure_modes.py`.
3. Any claim with a `withdraws` / `contradicts` / `supersedes` correction marks
   its asserting units `withdrawn`.
4. At query time a withdrawn unit is down-ranked below its own correction, and
   the correcting unit is **force-attached** to the result set. A withdrawn
   claim can never be returned bare.

Seeded with cas-9d92's two withdrawn claims: "there is no reconciliation code
path" (refuted by `prompt_queue_store.rs:4493`, which handles
`SurfacingSource::HookSurfaced`) and "rows reached through `inbox_poll` prove
acknowledgement is broken" (refuted by the documented non-ack contract at
`prompt_queue_store.rs:64`, `:4553`, and its regression test at `:9582`).

## Schema — the join surface M2 consumes

| Table | Role |
|---|---|
| `evidence_units` | one row per unique normalized text; carries type, scope, claim key, correction state, `embed_state` |
| `evidence_provenance` | one row per observation: source path/locator, session, task, worker, commit, file, symbol, timestamp, epoch, scope, host/project/team |
| `evidence_links` | the typed join rows — `task \| session \| worker \| commit \| file \| symbol \| epoch` — each with a confidence and a method |
| `evidence_corrections` | claim key, relation, authority, evidence pointer |
| `ingest_watermarks`, `ingest_runs` | resumability and per-run receipts |
| `redaction_receipts`, `retention_receipts` | privacy and deletion receipts |

`evidence_links.method` records how a link was established, so M2 can weight
them honestly instead of treating a prose guess as a fact:

| Method | Confidence | How |
|---|---:|---|
| `row-key` | 1.0 | the source row's own task/session identifier |
| `reference-join` | 1.0 | the token resolved against a known commit, file, or symbol |
| `commit-file-join`, `commit-symbol-join` | 0.9 | derived from a resolved commit's changed files/symbols |
| `history-epochs` | 1.0 | the deployed-binary epoch active at that instant |
| `basename-join`, `name-join` | 0.6 | a filename or bare identifier matched a known path or symbol |
| `text-extract` | 0.4 | path-shaped text with no corresponding known file |

## Division of labour: SQL for metrics, vectors for text

Structured metrics are derived by SQL only. `evidence_units.py reproduce` reruns
the cas-9d92 v1 findings — unprocessed `worker_died` notices, zero
`delivery_attempts`, undelivered rate by day, pending-reason attribution, and
epoch stratification — against the snapshot, and reports the withdrawn claims
alongside so they cannot re-enter as live findings.

Ingestion **never embeds**. Units are left `embed_state='pending'` for the M2
index lane, which owns embedding and hybrid ranking in its own namespace.

## Commands

```
evidence_units.py ingest --namespace-root <dir> [--scopes host,project,team] [--max-rows N] [--max-bytes N]
evidence_units.py status --namespace-root <dir>
evidence_units.py reproduce --namespace-root <dir> --snapshot <dir>/snapshot/cas.db
evidence_units.py retention --namespace-root <dir> --policy host=30,project=365,team=365
evidence_units.py query --namespace-root <dir> "<text>" [--scopes project]
```

## Measured on real sources, 2026-08-18

Not a fixture run. Sources: the live project coordination DB
(`Petrastella/cas-src/.cas/cas.db`, 631 MB, 1,001,021 events), 49 daemon logs,
and the Claude/Grok transcript roots.

| Run | Sources | Bytes read | Candidates | Unique units | Collapsed | Links | Wall |
|---|---:|---:|---:|---:|---:|---:|---:|
| First pass (budgeted) | 305 | 180,079,930 | 85,445 | 36,226 | 57.6% | 465,106 | 37.3 s |
| Immediate second pass | 11 | 10,020,925 | 9,772 | 476 | 95.1% | 25,540 | 7.1 s |

The second pass touched 11 sources rather than 305 — only the files that had
actually grown, all of them live daemon logs and in-flight transcripts. That is
the incrementality claim, measured.

The two reduction percentages differ for an honest reason. The first pass is
budget-limited and reads a broad, varied slice, so much of it is genuinely
novel. The second reads only the append tail of live logs, which is dominated by
poll-tick and redelivery repetition — the same shape cas-c505 measured at 92.17%
over the full historical corpus. Steady-state continuous operation looks like
the second row, not the first.

**Read-only proof at real scale.** 49 real daemon logs plus a 631 MB copy of the
real coordination DB were SHA-256 fingerprinted, ingested (23,372,652 bytes read,
29,518 units, 409,580 links), and re-fingerprinted: all 50 files byte-identical.

**cas-9d92 reproduction.** Rerun by SQL against the snapshot, compared with the
cas-c505 cutoff report:

| Baseline metric | cas-c505 at its cutoff | This run | |
|---|---:|---:|---|
| `worker_died` notices unprocessed | 2,081 / 2,294 | 2,081 / 2,305 | same backlog, 11 new notices |
| `prompt_queue.delivery_attempts = 0` | 3,292 / 3,330 | 3,668 / 3,706 | same 38 non-zero rows |
| `abandoned_unknown_target` | 42 | 42 | exact |
| `superseded_stale` | 2 | 2 | exact |

Mixed epochs are labelled and excluded from post-fix eligibility:
`mixed:2.63.0+2.64.0` (5,204 provenance rows) and `mixed:2.61.1+2.62.0` (3,125).

**Correction integrity, on real evidence.** Ingestion found 7 units touching
cas-9d92's withdrawn claims and adjudicated them into 2 assertions and 5
correction records. Querying the withdrawn claim's own wording now returns the
authoritative corrections at ranks 2 and 3 — the retrieval that cas-c505
measured as a safety failure, where neither lexical nor vector ranking surfaced
the correction at all. The two surviving assertions sink to ranks 399 and 400 of
400 and each carries all three correction records, including the current-source
pointer.

**Retention.** A `host=5,project=120,team=60` policy deleted 30,256 provenance
rows and 3,929 units, cleaned 82,234 join rows, left zero orphaned links,
preserved every correction unit, and wrote one hashed receipt per scope with the
oldest row retained.

### A bug this task's own subject caught

The first implementation of the correction markers matched only singular
"claim". cas-9d92's actual retraction reads *"Two claims WITHDRAWN…"* and the
cas-c505 brief reads *"WITHOUT reviving its two withdrawn claims"*. Both were
therefore filed as fresh assertions of the very claims they retire — the exact
failure mode this layer exists to prevent, reproduced in miniature by the layer
meant to prevent it. It surfaced only because the pipeline was run against the
real corpus rather than a tidied paraphrase. The markers now accept plurals and
the regression test uses the corpus's verbatim phrasing.

## Known limits

- Grok `chat_history.jsonl` carries no reliable per-row timestamp. Those units
  are ingested without one, are excluded from age-based retention, and are
  labelled `unattributed` for epoch. Deleting them on a guessed age would be a
  silent, unreceipted loss.
- Snapshot time versus row update time: a row created before a run but mutated
  between the snapshot copy and the query reflects the later state. Immutable
  event text and transcripts are unaffected; mutable queue-state columns carry
  this caveat, as they did for cas-c505.
- The claim registry is deliberately small and hand-curated. Lexical patterns
  nominate candidates; only a named authority retires a claim. Growing the
  registry automatically would reintroduce exactly the failure mode it exists to
  prevent.
