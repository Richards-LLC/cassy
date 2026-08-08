# Ambient recall benchmark — 2026-08-08

## Winner and margin

Bounded fusion won the fixed six-label harness: recall@3 improved from **2/6 (0.333) for BM25-only to 6/6 (1.000)**, a **+0.667 absolute** margin, while MRR improved from **0.500 to 1.000**. An authenticated isolated production-path run then confirmed the runtime contract: one query request per semantic recall event, Knowledge/History/Code fan-out from that vector, role-distinct ranking, zero private-canary hits, and a daemon drain that reached zero pending.

## Overview

| Measurement | Baseline / target | Result | Variance / verdict | Sample |
| --- | ---: | ---: | ---: | ---: |
| Recall@3 | BM25 0.333 | **Fusion 1.000, best** | **+0.667** | 6 labels |
| MRR | BM25 0.500 | **Fusion 1.000, best** | **+0.500** | 6 labels |
| Harmful / stale / leakage | 0 required | **0 / 0 / 0** | pass | 6 labels |
| Prompt p99 | 7,200 B role budget | **5,087 B** | −2,113 B | 6 corpus sizes |
| Cold semantic event | ≤1,500 ms study target | **247 ms** | −1,253 ms | 1 live event |
| Warm-local-cache supervisor event | ≤300 ms study target | **270 ms** | −30 ms | 1 live event |
| Query requests | 1 per semantic event | **2 / 2** | exact | 2 live events |
| Daemon freshness | ≤10 min active source/history target | **834 ms to pending 0** | under target | 4 live units |

Latency rows are single observations, not p95 estimates. Both include a remote query embedding; “warm-local-cache” means the SQLite/LMDB/cache handles were warm, not that the remote query call was skipped.

## Harness

- Code basis: authenticated harness commit `09b71cf3`; runtime replay checkpoint `b982bbd9`; Rust 1.95.0, test profile, one test thread.
- Fixed evaluation: six labels—two lexical pattern/decision queries, two semantic paraphrases (code/history), and two structural file/task bindings—with three lexical distractors per label.
- Prompt corpus: 1, 8, 32, 72, 1,000, and 10,000 candidates; source snippets exceed the injected 320-character card cap.
- Live run: a freshly migrated temporary git project; one Knowledge page, one History commit, and public/private Code symbols; configured `cas-cloud` / `cas-embed-v1` capability read in place. No credential was copied or printed and the live project store was not mutated.
- Held constant: canonical query construction, scope gate, role policies, evidence schema, deterministic ordering, candidate/injection caps, omission disclosure, and ledger behavior.
- Current SessionStart recall was not replayed numerically against these labels. The fixed comparison isolates BM25-only versus fusion; the live run validates production semantic transport and ranking, not a second quality corpus.

## Results

### Recall quality and bounded context

BM25-only retrieved the two lexical labels in its top three and placed the other four targets fourth. Fusion ranked all six targets first. The +0.667 recall margin came from the two paraphrase and two structural-binding labels; lexical labels did not regress.

Supervisor packet overhead measured 5,085 B p50, 5,087 B p95, and 5,087 B p99, 2,113 B below the 7,200 B default budget. The 10,000-candidate and separate ~750k-token-style cases remained bounded through candidate/card caps, reserved disclosure space, and hard truncation. Unchanged `(evidence_id, revision)` cards were not re-injected; a changed revision reappeared as a delta.

### Authenticated semantic runtime

The isolated daemon drain embedded four units in three provider requests and reached `pending_after=0` in 834 ms. It left an error-free History embeddings attempt at `2026-08-08T21:54:50.224939958Z` and error-free Code scan state at `2026-08-08T21:54:49.650311285Z`. The resulting cache held one Knowledge vector, one History vector, and two isolated Code vectors.

Two role events issued exactly two provider requests. The worker event completed in 247 ms and ranked `Code:sym-live` first; the supervisor event on warm local handles completed in 270 ms and ranked the History commit first. Both result sets contained Knowledge, History, and Code evidence. `sym-private-live` was absent from both, proving the authoritative scope check ran before vector comparison.

`cas-embed-v1` exposes neither billed token counts nor a monetary price. The honest cost receipt is therefore **3 drain requests for 4 units plus 2 query requests for 2 events**. Dollars remain parameterized:

```text
one query-event dollars = (T_query / 1,000,000) * P + Q
run dollars = (T_drain / 1,000,000) * P + 3Q + 2 * one query-event dollars
```

`P` is dollars per million billed input tokens and `Q` is any per-request charge. Neither is published, so no dollar amount is invented.

### Isolation and fallback

Adversarial fixtures reject foreign project/team rows and owner-unprovable private rows. Logged-out fallback opens existing SQLite read-only, creates neither Knowledge nor Code vector caches, and makes zero provider calls. Hook tests show cards do not become memories, nested internal-model hooks create no ambient activity, irrelevant transitions create no ledger, and unchanged results emit no dynamic context.

## Where the loser wins

BM25-only remains cheaper for exact lexical lookup or provider-absent operation: it needs no network request or vector cache. Fusion wins on paraphrase and structural binding. The live run proves one request per semantic event, not free semantic lookup; a later query-vector cache could reduce repeat cost but is not claimed by this receipt.

## Threats to validity

- Six deterministic labels establish ranker behavior, not population quality; current SessionStart was not numerically replayed.
- Live latency and freshness are single isolated observations, not p50/p95/p99 distributions.
- The live corpus intentionally contains four units to isolate namespace, role, scope, and call-count behavior; large-corpus prompt bounding is proven separately by deterministic tests.
- Prompt percentiles are enforced byte overhead using the conservative four-bytes-per-token boundary, not tokenizer-specific counts.
- Provider billing fields are absent. Requests, units, latency, and queue state are measured; dollars are formula-only.

## Provenance

- Fixed fixtures: `cas-cli/src/ambient_recall.rs`, tests `labeled_fusion_evaluation_beats_bm25_only_without_harmful_injection` and `prompt_overhead_percentiles_stay_bounded_across_large_corpora`.
- Live harness: `authenticated_isolated_live_provider_receipt` at commit `09b71cf3`; extraction window `2026-08-08T21:54:49Z–21:54:50Z`.
- Fixed command: `env ZIG=/home/pippenz/Petrastella/cas-src/.context/zig/zig cargo test -p cas --lib ambient_recall::tests -- --nocapture --test-threads=1`.
- Live command: `env ZIG=/home/pippenz/Petrastella/cas-src/.context/zig/zig CAS_AMBIENT_LIVE_CONFIG_DIR=/home/pippenz/Petrastella/cas-src/.cas cargo test -p cas --lib -- ambient_recall::tests::authenticated_isolated_live_provider_receipt --ignored --nocapture --test-threads=1`.
- Live result: exit 0; 1 passed, 0 failed, 4,220 filtered; test body 1.38 s. Fixed checkpoint result before the live harness: 15 passed, 0 failed, 4,205 filtered.
