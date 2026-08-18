# Operational index namespace + hybrid joins (cas-2556, M2 of cas-0cda)

Date: 2026-08-18 · Corpus cutoff inherited from cas-c505 (2026-08-17T13:44:57Z)
Tooling: `docs/analysis/scripts/operational_index.py`, tests in `docs/analysis/scripts/test_operational_index.py`
Machine-readable receipts: `docs/analysis/2026-08-18-operational-index-receipts.json`
Labelled set: `docs/analysis/operational-index-labels.json`

## What M2 delivers

A dedicated operational namespace (`operational/v2`) that holds only genuinely semantic
operational text, physically separate from the memory, knowledge and code indexes; a
labelled evaluation that decides — before anything is surfaced — whether the semantic
channel is allowed to answer at all; and hybrid queries that join operational events to
tasks, code symbols, commits, issues, memories and deployed-binary epochs with provenance
on every row.

Structured metrics stay in SQL. Nothing in this lane writes to any Cassy store: every store
handle is opened `mode=ro`, and `isolation-check` proves it by attempting a write and
recording the rejection.

## 1. The namespace

Built read-only from the frozen cas-c505 corpus into a standalone artifact at
`~/.cas/artifacts/cas-2556/operational-index/index.sqlite3` (291 MB).

| Stage | Count |
| --- | --- |
| Chunks considered | 80,951 |
| Admitted as semantic operational text | 46,654 |
| Collapsed after envelope stripping | 1,343 |
| Provenance occurrences | 88,098 |
| Vectors inherited (1024d, `cas-embed-v1`) | 46,654 (0 events without a vector) |

Admission rejections, by rule: `too-short` 11,535 · `structured-json` 10,701 ·
`key-value-telemetry` 6,998 · `low-prose-density` 3,697 · `low-lexical-variety` 23.

The admission policy strips structured envelopes and then judges the prose that remains.
`Task completed: Harden the liveness gate` is rejected — the envelope is a SQL fact and the
remainder is too thin to be semantic — while the same envelope wrapping a real progress
narrative is admitted with the envelope removed. Spawn-lifecycle telemetry and JSON
payloads never enter: those are the structured lane's material.

Admitted rows by source: codex transcripts 19,159 · claude transcripts 9,736 · events 7,722
· task prose 4,667 · daemon logs 2,761 · prompt queue 2,145 · grok transcripts 288 ·
supervisor queue 176.

Vectors are inherited from the frozen corpus rather than recomputed: same model, same
dimensions, zero new provider cost.

## 2. Isolation, proven in both directions

`operational_index.py isolation-check --live-fts` — all eight checks pass (exit 0):

- **Artifact is not a Cassy store or a store neighbour.** The namespace lives under the
  artifacts root, never in a store's own directory. The check failed on its first real run
  and that was correct behaviour, not a bug: it flagged a location inside `~/.cas` before
  the rule was narrowed to "not a store, not a store's directory, not registered".
- **Not registered in any Cassy store.** No store has `op_*` tables and no store metadata,
  knowledge-source, or index-state row mentions the artifact.
- **No memory/knowledge/code tables** inside the namespace; only `op_*` tables exist.
- **Every row namespaced** — 0 rows outside `operational/v2` across events, occurrences and
  vectors — and **no foreign source kinds** (memory/knowledge/code kinds are rejected at
  admission).
- **Operational text is invisible to memory, knowledge and code.** 25 canary phrases drawn
  from admitted rows, probed against `entries`, `knowledge_pages` and `code_symbols` in both
  the project and user stores: 0 hits.
- **Live full-text indexes return nothing operational.** `cas` exposes search over MCP, not
  as a CLI verb, so the probe goes at the substrate those surfaces read: 20 probes across
  `knowledge_pages_fts`, `history_commits_fts`, `recordings_fts` and `recording_text_fts`:
  0 hits.
- **Memory/knowledge/code text is absent from the namespace.** 75 canaries sampled from the
  three stores, matched by normalised content hash and by substring: 0 contamination.
- **Cassy stores are read-only.** A `CREATE TABLE` attempt on each store handle is rejected
  with `attempt to write a readonly database`.

The regression suite covers both failure directions: smuggling memory text into the
namespace, leaking operational text into `entries`, and parking the artifact next to a
store all make `isolation-check` exit 2.

## 3. The semantic gate

Seven probes were labelled by pooling the top 6 results of the prefix, lexical (BM25) and
vector channels and judging every pooled row on query intent alone, blind to the channel
that produced it. Gold is a reviewed **lower bound**; recall figures are comparative, not
absolute. Four of the seven probes have gold rows that a lexical baseline did surface, so
the baselines are not structurally doomed.

Averaged over each family at k=10 (recall@10 / MRR):

| Family | prefix | lexical | vector | hybrid |
| --- | --- | --- | --- | --- |
| instruction-drift | 0.167 / 0.167 | 0.167 / 0.333 | **0.833** / 0.289 | 0.500 / **0.381** |
| symptom-to-fix | 0.271 / 0.313 | 0.271 / 0.250 | **0.667** / **0.508** | 0.354 / 0.570 |

The honest result is a split. The semantic channel finds 3–5× more of the labelled evidence
than either baseline in both families. But on instruction drift its **ranking** loses to
BM25 (MRR 0.289 vs 0.333): probe D4's lexically-findable gold row sits at rank 1 for BM25
and far lower for vectors. So:

- **`vector` mode is not authorised.** The strict rule — the raw vector channel must beat
  both baselines on recall *and* MRR in every required family — fails on instruction drift.
- **`hybrid` mode is authorised.** The fused channel that actually answers beats both
  baselines on both metrics in both families, and the vector channel beats both baselines
  on recall.

`query`/`join` enforce this: `--mode vector` exits 3 with the per-family authorisation
printed; `--mode hybrid` answers. The gate receipt is stored in the index and is invalidated
by a corpus fingerprint change or an embedder mismatch, so a rebuilt corpus silently reverts
to baselines-only until it is re-evaluated. Probes cost ~1.2–1.4 s each (46,654 dot products
in pure Python plus two FTS queries).

Where the semantic channel earns its place: probe S3 ("the running program was an older
build, so a landed correction looked like it had not worked") returns **0.0** recall from
both baselines and **1.0** from vectors — including the transcript line
"the running daemon is `cas 2.50.0` built from `9114c1a` and installed at 18:55Z — my fix
landed at 23:45Z, so this close executes the *old* code". That is the deployed-binary-epoch
question M3 is built on, and no lexical query in the labelled set finds it.

## 4. Hybrid joins with provenance

`join` retrieves through the authorised channel and cross-references each event. Every row
carries namespace, source path, session, task, worker, timestamp, epoch and privacy scope;
every joined entity carries its store, join key and join method; `join` exits 4 if anything
is missing. All demonstration runs reported zero provenance violations at ~1.6–2.1 s.

Worked example — *"a worker kept breaking a standing rule it had just been told about"*:

- event 24881 (claude transcript): "the author of *never work into compaction* delivered the
  rule and then immediately began compacting, becoming the third casualty of the exact defect
  it had just documented"
- → task `cas-b4921` (task-id-mention): *Workers never foreground-block on long-running
  processes: guidance mandate + backgrounding recipe in all worker flavors (GH #121)*
- → issue **#121** twice over: mentioned in the text, and via that task's `external_ref`
- → memory `2026-08-09-19` (task-id-mention): the worker-context-budget mandate itself

Worked example — *"a panic traced into a specific rust source file…"*: event 2916 joins to
`cas-e202`, to commit `af7091c` (*fix(pty): codex keep-alive must not use tokio::spawn in
sync Pty::spawn*), and to symbols in `crates/cas-pty/src/pty.rs` — event ↔ symbol ↔ task ↔
commit in one row.

Join noise is treated as a correctness problem. The first implementation joined memories on
any long word ("installed", "processes") and symbols on any identifier ("process", "update",
"factory"), which manufactures evidence-shaped coincidence. Now symbol joins require a
distinctive identifier (snake_case or ≥12 chars) that resolves to at most three symbols, or
a path-segment file match; memory joins require session provenance, a task-id mention, or
**two** corpus-rare terms co-occurring in the same memory. Memories are surfaced with
`adjudication: surfaced only; contradiction adjudication is M4 (cas-2332)`.

## 5. Limits, stated

- **Epoch coverage.** `history_epochs` begins 2026-08-09, so events before that resolve to
  `unattributed` rather than to a guessed epoch — visible in the demonstration rows, and
  deliberate.
- **Labels are a lower bound.** Seven probes, pools of six per channel. Deeper pools and
  more probes would move the absolute numbers; they are comparative between channels on the
  same pool.
- **Vector ranking is O(corpus) in pure Python.** ~1.3 s per query at 46,654 rows is fine
  for sweeps and unacceptable for interactive use at 10× the corpus.
- **The corpus is frozen.** M2 reads cas-c505's snapshot; continuous ingestion is M1
  (cas-b78b). When that lands, `build` should consume its evidence units instead — the
  admission policy, isolation checks and gate carry over unchanged.
- **`hashing-test` embedder is test-only.** The gate records the embedder and refuses to
  authorise answers when it does not match the index's.

## Reproducing

```
python3 docs/analysis/scripts/operational_index.py build
python3 docs/analysis/scripts/operational_index.py isolation-check --live-fts
python3 docs/analysis/scripts/operational_index.py evaluate --labels docs/analysis/operational-index-labels.json --top 10 --record-gate
python3 docs/analysis/scripts/operational_index.py join "a worker kept breaking a standing rule it had just been told about"
python3 -m unittest docs/analysis/scripts/test_operational_index.py
```
