# Ambient recall benchmark — 2026-08-08

## Winner and margin

The bounded fusion ranker won the fixed six-label harness: recall@3 improved from **2/6 (0.333) for BM25-only to 6/6 (1.000)**, a **+0.667 absolute** margin, while MRR improved from **0.500 to 1.000**. The prompt boundary stayed bounded: supervisor packet overhead was **5,085 bytes p50, 5,087 bytes p95, and 5,087 bytes p99** against a 7,200-byte role budget.

These are deterministic fixture results, not a claim that a live cloud embedding provider was exercised. The production hook path in this checkpoint uses the capability-absent, read-only lexical/structural fallback; the fusion evaluation supplies labeled semantic scores directly to isolate ranking behavior.

## Overview

| Candidate | Recall@3 | MRR | Harmful | Stale | Leakage | Prompt p99 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| BM25-only baseline | 0.333 (2/6) | 0.500 | — | — | — | — |
| Bounded fusion | **1.000 (6/6), best** | **1.000, best** | 0/6 | 0/6 | 0/6 | 5,087 B |

Absolute recall variance versus BM25-only: **+0.667**. MRR variance: **+0.500**. Sample size: six labeled queries. Prompt-overhead sample size: six corpus sizes (1, 8, 32, 72, 1,000, and 10,000 compact candidates).

## Harness

- Commit: `27829c400e02e0874e13a8817b4b7dba3a8af556`.
- Rust: 1.95.0; test profile, one test thread for environment-sensitive hook tests.
- Dataset: six fixed labels — two lexical pattern/decision queries, two semantic paraphrase queries (code/history), and two structural file/task bindings. Each label includes three lexical distractors.
- Ranking: baseline sorts lexical score only; fusion weights lexical, semantic, structural binding, and role signals. Exact binding is independently protected by the production candidate ordering.
- Prompt corpus: six deterministic sizes through 10,000 candidates; each source snippet is deliberately larger than the injected 320-character card field.
- Scope negatives: foreign project, foreign team, and owner-unprovable private rows; all must be absent before ranking.
- Held constant: label set, distractors, deterministic tie-breaking, card schema, role budget, injection cap, and omission disclosure.
- Excluded: live provider latency/cost, live semantic cache quality, daemon freshness, and current-SessionStart recall quality. These require an authenticated isolated live run and must not be inferred from this fixture.

## Results

### Recall quality

BM25-only retrieved the two lexical labels in its top three and placed the other four targets fourth, giving recall@3 0.333 and MRR 0.500. Fusion ranked all six targets first, giving recall@3 1.000 and MRR 1.000. The +0.667 recall margin comes from the two paraphrase and two structural-binding labels; the lexical labels did not regress.

### Prompt overhead and repetition

The supervisor packet stayed at 5,087 bytes p99, 2,113 bytes below its 7,200-byte default budget. The 10,000-candidate case did not expand the boundary: fixed candidate and injection caps, 320-character snippets, reserved disclosure space, and hard truncation are applied before rendering. A separate 750k-token-style corpus test also stays within the role cap and reports omitted counts deterministically.

Revision-aware ledger tests prove that an unchanged `(evidence_id, revision)` is not injected twice, while a changed revision reappears as a delta. Ledger loss only causes safe repetition; it cannot suppress truth or create a durable memory.

### Isolation and fallback

Adversarial fixtures reject foreign project/team rows and owner-unprovable private rows. The logged-out fallback opens the existing `cas.db` read-only and creates neither `index/code-vectors` nor a provider call. Hook tests show injected cards do not increase the memory count, and irrelevant short transitions do not create a ledger.

## Where the loser wins

BM25-only remains the cheaper channel when a query is an exact lexical lookup and semantic capability is unavailable or undesirable: it needs no provider, vector cache, or query embedding. The fusion advantage appears on paraphrase and structural-binding labels. This checkpoint preserves that cheap path as the production fallback; it does not yet provide a live authenticated receipt proving one shared query vector fans out across knowledge, history, and code namespaces.

## Threats to validity

- The six-label corpus is intentionally small and deterministic; it establishes ranker behavior, not population-level quality.
- Semantic scores are fixture inputs. They do not measure a provider/model's embedding quality.
- Prompt percentiles are byte overhead (the enforced boundary), not tokenizer-specific counts; the runtime budget uses the conservative four-bytes-per-token estimate.
- Current SessionStart was not replayed against the same labels, so no numeric claim is made against that baseline.
- No authenticated isolated live run was performed; provider latency, query-call count, monetary cost, and daemon freshness remain unmeasured.

## Provenance

- Source fixture: `cas-cli/src/ambient_recall.rs`, tests `labeled_fusion_evaluation_beats_bm25_only_without_harmful_injection` and `prompt_overhead_percentiles_stay_bounded_across_large_corpora`.
- Extraction time: 2026-08-08T21:29:41Z.
- Exact command:

  `env ZIG=/home/pippenz/Petrastella/cas-src/.context/zig/zig cargo test -p cas --lib ambient_recall::tests -- --nocapture --test-threads=1`

- Fresh result: 13 passed, 0 failed, 4,205 filtered; evaluation line `labels=6 bm25_recall_at_3=2/6 fusion_recall_at_3=6/6 bm25_mrr=0.500 fusion_mrr=1.000 harmful=0 stale=0 leakage=0`; overhead line `samples=6 p50_bytes=5085 p95_bytes=5087 p99_bytes=5087 hard_cap_bytes=7200`.
