# Historical Cassy operational vector index

<p class="lead"><strong>Headline finding.</strong> Structural normalization removed <strong>92.17%</strong> of 1,033,752 candidate evidence chunks before embedding, turning 3.03 GB of Cassy operational sources into 80,951 provenance-bearing semantic units without adding anything to live Cassy search.</p>

<div class="cards">
<div class="card"><strong>3.03 GB</strong>eligible frozen source data<br><small>cut off 2026-08-17 13:44:57 UTC</small></div>
<div class="card"><strong>80,951</strong>unique semantic chunks<br><small>from 1,033,752 candidates</small></div>
<div class="card"><strong>92.17% ↓</strong>candidate reduction<br><small>952,801 repeats collapsed</small></div>
<div class="card"><strong>32,433</strong>redactions<br><small>secrets and email addresses before embedding</small></div>
</div>

## Overview

| Metric | Current | Baseline | Absolute variance | Percent variance | Sample |
|---|---:|---:|---:|---:|---:|
| Eligible source bytes | 3,032,566,716 | 1,000,000,000 task estimate | +2,032,566,716 | +203.26% | 1 snapshot + 1,510 files |
| Unique chunks sent for embedding | 80,951 | 1,033,752 candidates | -952,801 | -92.17% | 1,033,752 candidates |
| Estimated embedding tokens | 13,530,757 | unfiltered estimate¹ | -744,744,243 | -98.22% | 54,123,027 retained characters |
| Provider-list-price equivalent | $1.759 | $97.27 unfiltered¹ | -$95.51 | -98.19% | $0.13 / million tokens |

¹ The unfiltered estimate uses the deliberately conservative four-bytes-per-token approximation over 3.03 GB. It is a comparison model, not a measured tokenizer count. Sources: `full-prepare.json`; [OpenAI's `text-embedding-3-large` model page](https://platform.openai.com/docs/models/text-embedding-3-large), checked 2026-08-17.

<figure aria-describedby="reduction-summary">
<svg viewBox="0 0 820 180" role="img" aria-labelledby="reduction-title reduction-desc">
<title id="reduction-title">Normalization removed 92.17% of candidate chunks before embedding</title>
<desc id="reduction-desc">An outlined baseline bar represents 1,033,752 candidate chunks. A solid bar occupies 7.83 percent of that width and represents the 80,951 unique chunks retained for embedding.</desc>
<rect class="bar-removed" x="170" y="48" width="600" height="42" rx="3"/>
<rect class="bar-kept" x="170" y="48" width="47" height="42" rx="3"/>
<text class="chart-label" x="10" y="74">Candidates</text><text class="chart-label" x="780" y="74" text-anchor="end">1,033,752</text>
<path class="zero" d="M217 40 V115"/><text class="chart-label" x="225" y="112">80,951 kept (7.83%)</text>
<text class="chart-label" x="170" y="145">952,801 collapsed before embedding (92.17% reduction)</text>
</svg>
<figcaption id="reduction-summary">Candidate-to-unique variance. Source: `historical_vector_index.py prepare`, frozen cutoff 2026-08-17T13:44:57Z; extracted 2026-08-17.</figcaption>
</figure>

| Stage | Chunks | Share of candidate baseline | Encoding |
|---|---:|---:|---|
| Semantic candidates after structural filtering | 1,033,752 | 100.00% | outlined baseline |
| Repeats/boilerplate/storm instances collapsed | 952,801 | 92.17% | removed variance |
| Unique chunks retained | **80,951** | **7.83%** | solid actual |

## Method

### Fixed cutoff and read-only snapshot

The corpus cutoff is **2026-08-17T13:44:57Z**. The live coordination database was never queried for analysis and was never written. `cas.db`, `cas.db-wal`, and `cas.db-shm` were filesystem-copied to the task artifact root, opened with SQLite `mode=ro`, and passed `PRAGMA integrity_check` (`ok`). SHA-256 receipts:

- database: `718709c7eff20f48c16c547337b16addbf81aba27c5079e362124128a065c786`
- WAL: `94dd799bff9b28775949c51b3b3068c7376cc0a6a462b378e6f7ae25d243f2a8`
- SHM: `82041a3b79f1ab2b738cc44304d917e67d44a8bde9395a60623fa248b1e62ca6`

Rows and transcript/log events after the cutoff are excluded by timestamp. Grok's canonical chat history does not carry reliable per-row timestamps; its files were complete before the cutoff and this limitation is retained in provenance as `unattributed` rather than assigned a guessed epoch.

### Source inventory before filtering

| Source | Discovery population | Eligible canonical population | Bytes embedded from |
|---|---:|---:|---:|
| Coordination snapshot (`db` + WAL + SHM) | 3 files | 3 files | 637,929,328 |
| Claude Cassy transcripts | 628 files | 628 files | 464,310,658 |
| Codex transcript store | 2,018 files / 4,685,246,440 bytes | 821 Cassy-cwd sessions | 1,888,567,587 |
| Grok Cassy transcript representations | 71 files / 100,708,411 bytes | 13 canonical `chat_history.jsonl` files | 10,682,798 |
| Daemon/factory logs | 48 files | 48 files | 31,076,345 |
| **Total canonical input** |  | **1,510 files + snapshot** | **3,032,566,716** |

Codex eligibility is determined from `session_meta.cwd` under `/Petrastella/cas-src`; other projects are inventoried but never parsed into the index. Grok's 58 `events`, `updates`, and `rewind_points` files (90,025,613 bytes) are alternate serializations of the 13 canonical chat histories and are structurally excluded before text extraction.

### Structural preprocessing

The reproducible builder performs these operations in order:

1. Extract natural-language task, task-note/event, delivery message, canonical user/assistant transcript, and log-message text while excluding system/developer/tool payloads.
2. Strip deterministic harness blocks (`skills_instructions`, permissions, environment, git status, system reminders) and transport provenance wrappers.
3. Redact bearer/API tokens, private-key blocks, explicit password/secret assignments, and email addresses.
4. Split retained text into at most 3,600-character chunks with 240-character overlap.
5. Normalize volatile hashes, identifiers, durations, numeric counts, and absolute paths for the deduplication key; preserve the unnormalized redacted text for retrieval.
6. Store one `chunks` row per unique normalized hash and one `occurrences` row for every source occurrence. This collapses storms without discarding provenance or frequency.
7. Attach source, session, task, worker, timestamp, privacy scope, and deployed-binary epoch to every occurrence. Mixed-version intervals are labeled `mixed:<versions>`, not treated as post-fix.

The index is a standalone SQLite artifact at `~/.cas/artifacts/cas-c505/frozen-index/index.sqlite3`. It is never registered beneath project `.cas/index`, the personal/global knowledge cache, the history namespace, or the code-vector cache. It therefore cannot contribute hits to live memory, knowledge, history, or code search.

### Privacy and embedding receipt

The explicit task authorization covers one fixed historical embedding build. Text is redacted locally before the request. The Cassy cloud contract maps `cas-embed-v1` to OpenAI `text-embedding-3-large` at 1,024 dimensions; the committed cloud response states that the endpoint persists neither text nor vectors and retains only request metadata (count, model, duration). The local artifact retains the returned vectors and the project-private provenance. It must not be published outside the task artifact boundary.

## Results

### Corpus reduction by source

| First-seen source kind | Unique chunks | Source occurrences | Collapse signal |
|---|---:|---:|---:|
| Codex transcript | 22,486 | 25,798 | 12.84% fewer unique than occurrences |
| Daemon log | 16,358 | 70,627 | 76.84% fewer unique than occurrences |
| Coordination event | 15,937 | 882,733 | 98.19% fewer unique than occurrences |
| Claude transcript | 13,518 | 41,646 | 67.54% fewer unique than occurrences |
| Task | 5,183 | 5,185 | 0.04% fewer unique than occurrences |
| Supervisor queue | 3,720 | 3,852 | 3.43% fewer unique than occurrences |
| Prompt queue | 3,205 | 3,367 | 4.81% fewer unique than occurrences |
| Grok transcript | 544 | 544 | 0.00% fewer unique than occurrences |
| **Total** | **80,951** | **1,033,752** | **92.17% fewer unique than occurrences** |

The event table supplies 85.39% of occurrences but only 19.69% of unique chunks. That is the quantitative reason structural deduplication had to precede embedding: raw event counts would have spent almost the entire budget on repeated lifecycle text.

### Index build and retrieval performance

| Measure | Result | Receipt |
|---|---:|---|
| Embedded chunks | 80,951 / 80,951 | zero pending; SQLite integrity `ok` |
| Successful embedding requests | 2,530 | maximum 32 inputs/request |
| Provider failures | 1 transient HTTP 502 | failed batch wrote nothing; resume completed 823 pending chunks in 26 requests |
| Full embedding wall time | 53 min 11 sec | 52:39.83 initial run + 30.96 sec resumable tail |
| Frozen index size | 616,759,296 bytes | 588.19 MiB standalone SQLite file |
| Local full-index ranking latency | 2.23–2.54 sec/query | median 2.27 sec over eight labels; query-embedding network time excluded |
| Evaluation run | 24.16 sec | one batch query-embedding request + eight lexical/vector/hybrid ranks |

The only provider failure occurred after 80,128 vectors had committed. Because a batch writes vectors and `embedded=1` flags in one SQLite transaction, the failed batch left 823 chunks pending and the resume embedded only those chunks. The builder now applies bounded exponential retry (four retries after the first attempt) in addition to resumability.

### Labeled lexical, vector, and hybrid queries

| Label | Lexical lead | Vector lead | Hybrid judgment |
|---|---|---|---|
| Silent delivery and replay | live transcript evidence (`cas-20ac`) and task `cas-6ad2` | lifecycle events for `cas-6ad2` | **Best:** task narrative plus two independent acknowledged-then-replayed samples |
| Poll-tick redelivery storm | exact GH #166 task `cas-5c50` | adjacent idle-delivery tasks `cas-893c`/`cas-977e` | **Best:** exact storm plus precursor `cas-ceae`; dense alone broadened too far |
| Instruction-prefix drift | exact hardcoded-prefix task `cas-ba76` | adjacent “tool loaded but not callable” task `cas-e7c8` | **Mixed:** hybrid linked obsolete manifest spelling and harness prompt drift, but lexical was more precise |
| Merge/close/amendment race | cas-9d92 premature-close correction and lost amendments | GH #101 task `cas-f02b` on missing MERGE REQUIRED delivery | **Best:** combines the concrete cas-9d92 incident with the merge-rejection family |
| Parallel migration collision | cas-6212 ledger narrative | cas-6212, concurrent migration transaction evidence, and guard task `cas-b4bb` | **Best:** promoted the actual uniqueness-guard fix to hybrid rank 2 |
| Missed harness mirror | transcript statements about aligned mirrors | capability-parity task `cas-cc8c` | **Best:** combined the parity epic with wrong-prefix task `cas-2c61` |
| Workspace-contract conflict | repeated sanctioned-fallback transcripts | the same recovery family | **Equivalent:** exact vocabulary already made lexical sufficient; no dense-only gain |
| Withdrawn missing-code-path claim | unrelated reconciliation prose | adjacent “evidence not reproduction” and later-defect caveats | **Safety failure:** neither ranker surfaced the authoritative correction; current source inspection was required |

In 31 of 40 vector top-five chunk positions, the exact chunk was absent from the lexical top 50. That number measures *novel chunks*, not 31 proven new incidents: lifecycle rows for a task can be different chunks from its task narrative. Human adjudication found genuine cross-wording value in the merge, migration, and mirror families; it also found dense over-broadening in the storm/prefix queries and a correction-retrieval failure in the withdrawn-claim query.

### What semantic search added

SQL, grep, and exact-hash baselines remain superior for counts, state transitions, and duplicate rates. Semantic search added three narrower capabilities:

1. **Symptom-family expansion.** “Close before merge, then discover amendments” retrieved both the cas-9d92 incident and the separately worded GH #101 missing-MERGE-REQUIRED family. Exact SQL can count either state, but it cannot propose that relationship.
2. **Observation → guard association.** The parallel-lane narrative retrieved `cas-b4bb`, the build-time uniqueness guard, without requiring the query to name migration 222, GH #181, or the test function. Hybrid ranking kept the exact cas-6212 ledger first and raised the guard to second.
3. **Cross-harness drift association.** The mirror query joined worker statements about keeping copies aligned, the `cas-cc8c` capability-parity task, and the `cas-2c61` wrong-prefix task. The common concept is harness divergence, not a shared exact phrase.

What it did **not** add is equally important. Dense retrieval did not reliably distinguish a repeated withdrawn claim from its later correction. The correction was established only by current source (`SurfacingSource::HookSurfaced`) and the explicit inbox-poll non-ack contract. The operational index is therefore a candidate generator; hybrid/current-code evidence is the decision surface.

### Cross-channel association

The frozen operational index is intentionally only one evidence channel. The labeled queries were also checked against the existing Cassy surfaces:

| Operational association | Memory/knowledge | Task/issue | Code/symbol | Commit/provenance |
|---|---|---|---|---|
| Harness-prefix drift | memory `2026-08-06-19`; knowledge page on three-harness diaries | `cas-2c61`, `cas-48aa`, `cas-703a` | harness-aware prefix guards and builtin parity tests | `0b074b23` fixed ~25 Codex skill files; `1cda0319` made the wrong change and `8fd6cecf` reverted it two minutes later |
| Parallel migration-ID collision | memory `2026-08-08-269` on parallel same-file lanes | cas-6212 migration ledger notes; existing GH #181 recommendation | `Migration.id` and `test_migration_ids_unique` (`crates/cas-core/src/migration/migrations/mod.rs`) | history search exposes migration commits but only 15.26% exact task-provenance coverage |
| Silent delivery / replay | cas-9d92 decision and correction record | GH #155/#160 and tasks `cas-7787`, `cas-9d92` | `SurfacingSource::HookSurfaced` and inbox-poll non-ack contract | history provenance distinguishes fixes from later correction commits |
| Missed harness mirror | memory `2026-08-06-19`; harness diary knowledge | `cas-703a` flavor-drift guard, `cas-cc8c` capability parity | three-way builtin catalogs/tests | `da6c52ee` adds the three-harness parity manifest |

This composition is more useful than concatenating every corpus into one vector store: operational similarity finds the symptom family, while the existing structured channels establish current source truth, the fixing commit, task/issue ownership, and provenance confidence.

### cas-9d92 baseline reproduction and correction integrity

The inherited SQL/grep baseline was rerun on the cutoff snapshot. Current retained rows show:

| Baseline query | Cutoff result | Interpretation |
|---|---:|---|
| `worker_died` supervisor notices unprocessed | 2,081 / 2,294 (90.71%) | the backlog shape remains measurable; dedupe/issue review required before any new filing |
| `prompt_queue.delivery_attempts = 0` | 3,292 / 3,330 (98.86%) | reproduces the dead/rare instrumentation shape on the retained queue |
| Undelivered messages on 2026-08-13 | 90 / 1,199 (7.5%) | later days fall to 0–0.1%; absence in a short window is not proof of resolution |
| Most repeated message in 2026-08-17 log | 3 lines | the 704,901-line 2026-08-06 storm is not present in retained current logs; the committed cas-9d92 report remains its durable evidence |

Two cas-9d92 claims are explicitly **not reproduced**:

- “There is no reconciliation code path” is withdrawn. Current source at `crates/cas-store/src/prompt_queue_store.rs:4493` handles `SurfacingSource::HookSurfaced` and stamps `acked_via='hook_surfaced'`.
- Rows obtained through `inbox_poll` do not prove acknowledgement is broken. The current source contract explicitly says that inbox polling does not acknowledge (`prompt_queue_store.rs:64`, `:4553`, and its regression test at `:9582`).

This matters to semantic analysis: repeated historical prose can rank a withdrawn claim highly. A vector hit is a lead, not a verdict; current code and later correction events outrank similarity.

### Ranked association recommendations after dedupe

1. **Fold correction-aware evidence units into open task `cas-b78b` (P1 recommendation).** Add `supersedes` / `withdrawn` / `contradicted_by` metadata and down-rank a claim when a later correction explicitly names it. Evidence: the eighth labeled query failed to retrieve cas-9d92's authoritative correction even though the corpus contains it. Dedupe: `cas-b78b` already owns evidence-unit normalization and continuous read-only ingestion, so filing another task would split the same layer.
2. **Validate assignment artifact paths against the factory workspace contract (P2 new-task recommendation).** At task assignment/spawn, reject or rewrite prescribed output paths that are outside the worktree, task artifact root, or harness scratchpad; surface the resolved `artifacts_root` in the brief. Evidence: this task prescribed `/mnt/datacube`, the gate correctly blocked it, and the supervisor confirmed the brief predated the gate. Dedupe: closed #196/#201/#203 cover gate enforcement and sanctioned roots, not stale paths embedded in task briefs.
3. **Keep the delivery family closed unless new binary-epoch evidence reproduces it (covered; no issue).** The index strongly clusters #70/#75/#119/#123/#124/#130/#155/#160/#165/#166/#390. Current retained SQL still shows large unprocessed/zero-attempt populations, but those fields alone do not prove the old mechanism; cas-9d92 demonstrated why inferring mechanism from missing data is unsafe.
4. **No migration-collision issue (covered by #181 / `cas-b4bb`).** The semantic association correctly found the guard and current source has `test_migration_ids_unique`. A new issue would be a duplicate.
5. **No harness-mirror issue (covered by #116/#302, `cas-2c61`, `cas-703a`, `cas-cc8c`).** The current three-way parity manifests and prefix guards are the remediation. Future observations should first prove a current guard escape.
6. **Reuse the 92.17% structural reduction in `cas-b78b`, but do not turn this artifact into a watcher.** The one-time index remains frozen by design. Any later continuous lane must rebuild evidence from read-only sources, retain correction metadata, and use a separate namespace; it must not append to this cutoff artifact.

## Threats to validity

- **Corpus growth:** the task was estimated at about 1 GB; the fixed-cutoff eligible corpus is 3.03 GB because Codex and factory history grew after task creation. The inventory states both the discovery population and the selected Cassy population.
- **Snapshot time versus row update time:** cutoff filters are applied to event creation timestamps. A row created before the cutoff but mutated between the cutoff and filesystem copy may reflect later state. Immutable historical event text and transcripts are unaffected; mutable queue-state metrics carry this caveat.
- **Grok timestamps:** canonical Grok chat rows lack reliable timestamps. They preserve session/source provenance and an `unattributed` epoch instead of a fabricated timestamp.
- **Approximate tokens and list price:** token count uses characters divided by four and cost uses the direct provider list price. The Cassy service may meter differently; both are labeled estimates.
- **Dense similarity is non-causal:** high cosine similarity associates descriptions, not mechanisms. Recommendations require corroboration from SQL, current code, task/issue state, and change history.
- **History provenance coverage:** the history surface reports 557/3,651 exact task edges (15.26%) and 918/3,651 commits with any populated edge (25.14%). Missing provenance is returned honestly rather than treated as a negative association.
- **No continuous refresh:** the artifact ends at the fixed cutoff. Later fixes or regressions must be checked in live structured channels; this index deliberately has no watcher, daemon hook, or model turn.

## Provenance and reproduction

- Markdown source: `docs/analysis/2026-08-17-historical-operational-vector-index.md`
- HTML review surface: `docs/analysis/2026-08-17-historical-operational-vector-index.html`
- Builder/query script: `docs/analysis/scripts/historical_vector_index.py`
- Labeled query manifest: `docs/analysis/scripts/historical_vector_queries.json`
- Renderer: `docs/analysis/scripts/render_historical_vector_report.py`
- Frozen index and receipts: `~/.cas/artifacts/cas-c505/frozen-index/` and sibling JSON/text receipts
- Source commit at analysis start: `a8052bcf20be8dca7153fc155e213bbc31ff5a23`
- Worker implementation branch: `factory/swift-puma-69`
- Data window: earliest retained source through **2026-08-17T13:44:57Z**

Reproduction commands:

```bash
python3 docs/analysis/scripts/historical_vector_index.py inventory
python3 docs/analysis/scripts/historical_vector_index.py prepare
python3 docs/analysis/scripts/historical_vector_index.py embed
python3 docs/analysis/scripts/historical_vector_index.py evaluate \
  --queries docs/analysis/scripts/historical_vector_queries.json
python3 docs/analysis/scripts/render_historical_vector_report.py \
  docs/analysis/2026-08-17-historical-operational-vector-index.md \
  docs/analysis/2026-08-17-historical-operational-vector-index.html
```

The Exa research pass adopted two practices: template/deduplication before vectorization ([iPACK, arXiv:2302.09520](https://arxiv.org/abs/2302.09520)) and hybrid lexical+dense retrieval with structured evidence kept separate ([LogSage, arXiv:2506.03691](https://arxiv.org/abs/2506.03691)). The price receipt uses the official OpenAI model page for `text-embedding-3-large`, retrieved 2026-08-17. These sources informed the method; all Cassy findings above come from the frozen local corpus and committed code/history channels.
