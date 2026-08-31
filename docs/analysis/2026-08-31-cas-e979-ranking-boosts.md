# cas-e979 — the `apply_boosts` ranking knobs: measured, then deleted

**Date:** 2026-08-31 · **Task:** cas-e979 (EPIC cas-8fac) · **Measured by:** fast-robin-31
**Harness:** `cas-cli/tests/retrieval_eval_test.rs` (cas-e0ed / cas-b06c), 56 hand-judged
prompt-contexts over 189 real memory entries.

## Verdict

`SearchOptions::boost_feedback` / `boost_recency` / `boost_importance` and the
`apply_boosts` function they gated have been **deleted**.

Not because the boosts were a bad idea, and not because they measured worse — but
because **their output was never read**. `apply_boosts` computed a boosted score and
the only production consumer discarded it one line later. No configuration of the
three flags could move any metric, so the audit's decision rule ("if no configuration
improves at least one metric, delete") resolves to deletion.

Deleting them did not move the committed baseline by a single digit. That is the
cleanest possible confirmation that the removed code was dead.

## The mechanism

Each step below was verified by direct probe, not inferred.

1. `SearchIndex::search` computed `apply_boosts(bm25_score, entry, opts)` and wrote it
   into **both** `SearchResult::score` and `SearchResult::boosted_score`, leaving
   `bm25_score` as the raw BM25 value. It then re-sorted by the boosted score and
   calibrated the result into 0–1.

   At this level the boosts genuinely worked. Probing the fixture with the query
   *"release tag workflow announced digests linux"*:

   | arm | 2nd hit score | 4th hit |
   |---|---|---|
   | off | `2026-08-20-3` = 0.52502 | `2026-07-06-6` |
   | importance | `2026-08-20-3` = **0.56540** | **`2026-08-14-15`** |
   | feedback | byte-identical to off | byte-identical to off |

   The importance arm changed both the value **and** the ordering.

2. `HybridSearch::search` — the only production consumer — then did:

   ```
   let bm25_results = self.bm25_index.search(&bm25_opts, entries)?;
   let bm25_scores = bm25_results.iter().map(|r| (r.id.clone(), r.bm25_score)).collect();
   ```

   It read **`bm25_score`**, the raw field. The boosted value was discarded. The
   boost-induced reordering was discarded too, because the results are mapped to
   `(id, score)` pairs and re-fused downstream. `results.truncate(opts.limit)` could
   in principle have preserved a membership effect, but the production caller passes
   `limit = entries.len() * 2`, so truncation never bites.

3. There is no production caller of `SearchIndex::search` outside `HybridSearch`, and
   `search_unified` — what the `mcp__cas__search` tool uses — never called
   `apply_boosts` at all.

**Conclusion:** on the only production path that reached `apply_boosts`, the three
flags were structurally inert. Not "no measurable effect" — the computed value was
never read by anything.

## Measured table — as shipped

Selector `helpful_memories_production` (the real SessionStart path). Every arm was
also run with `enable_temporal` off, to keep the temporal channel from confounding
the recency arm.

| config | tier | query | P@5 | R@5 | lenP@5 | hit | distinct |
|---|---|---|---|---|---|---|---|
| off, feedback, recency, importance, all three — temporal on **or** off | all_working | seeded_task | 0.0500 | 0.0521 | 0.0857 | 10/56 | 51 |
| every arm | live_tiers | seeded_task | 0.0107 | 0.0096 | 0.0393 | 2/56 | 43 |
| every arm | all_working | fresh_session | 0.0071 | 0.0051 | 0.0321 | 1/56 | 1 |

Ten arms, three mode combinations, **one distinct result per combination**.

That the plumbing was live — and so that this null is real rather than a wiring
failure in the experiment — is proven by the temporal control: flipping
`enable_temporal` to false through the *same* options struct changes the BM25 result
set (189 → 185 hits) and its entire head.

The ambient selectors (`ambient_packet`, `ambient_candidates`) are **measured
invariants** here, not decision inputs: `ambient_recall.rs` never enters
`hybrid_search` at all, so no boost flag could reach them.

## Counterfactual — what if the boosts had been wired?

`hybrid.rs` was temporarily patched to read `r.score` instead of `r.bm25_score`, the
matrix re-run, and the patch reverted.

| config | P@5 | R@5 | hit |
|---|---|---|---|
| off | 0.0500 | 0.0521 | 10 |
| feedback | 0.0500 | 0.0521 | 10 |
| recency | 0.0500 | 0.0521 | 10 |
| **importance** | **0.0536** | **0.0557** | **11** |
| all three / temporal **on** | 0.0500 | 0.0521 | 10 |
| all three / temporal **off** | 0.0536 | 0.0557 | 11 |

Three things this says, with the caveats attached rather than buried:

- **Importance is the only arm with any signal** (+7.2% P@5, +6.9% R@5, no regression).
- **That signal is one case out of 56 flipping** (10 → 11 hits). On a hand-labeled
  fixture that is inside the noise a re-labeling pass would produce. It is not, on its
  own, a defensible basis for changing a production default.
- **Recency and the temporal channel interact destructively.** All three boosts score
  0.0500 with temporal on but 0.0536 with temporal off — the recency boost cancels
  importance's gain. A single-configuration A/B would have reported "all boosts do
  nothing" and hidden the interaction entirely.

Note also that switching the field read is **not** a pure "wire up the boost" change:
`bm25_score` is raw BM25 (~30) and `score` is calibrated 0–1, so it also changes the
BM25 channel's scale going into fusion. That deserves its own review.

## Corpus-limited vs code-limited inertness

The audit asked for these to be distinguished, because they have different remedies.

- **`feedback` is corpus-limited.** Only 4 of 189 fixture entries have
  `helpful_count > 0` and **zero** have `harmful_count > 0`. The multiplier
  `(1 + 0.1·helpful) · max(0.1, 1 − 0.1·harmful)` is therefore ≈1.0 for 185/189
  entries. Even correctly wired, this arm had almost nothing to act on.
  **Re-open condition:** revisit once cas-8f93's outcome attribution actually
  populates `helpful_count` / `harmful_count` at scale. Until then, any feedback-boost
  verdict measured on this corpus is a statement about the corpus, not the code.
- **`importance` is code-limited, not corpus-limited.** 158 of 189 entries have
  `importance > 0.5` and none below, so the `0.5 + importance` multiplier spread them
  over 1.0–1.5×. It was inert purely because of the discarded field read.
- **`recency` is neither** — it is inert alone, but interacts with the temporal
  channel as shown above.

## Open follow-up questions (deliberately not decided here)

1. **Should the discarded field read be fixed?** Filed as **cas-e7ae** (P3,
   main-targeted), framed as *defect, not dead feature*. `HybridSearch::search`
   reading `bm25_score` rather than `score` is the reason this whole knob was dead.
   Fixing it is a fusion-behaviour change (raw ~30-scale → calibrated 0–1) and needs
   its own review and a re-run of this matrix. The counterfactual table above is the
   input to that decision.
2. **Should equivalent boosts apply in `search_unified`?** An "enable" decision would
   only ever have moved the SessionStart Helpful-Memories path — `search_unified`,
   which backs the `mcp__cas__search` tool, never called `apply_boosts`. Whether
   agent-facing search *should* rank by feedback/recency/importance is a separate
   product question and is explicitly **not** settled by this task.
3. **Does any of this matter before tier starvation is fixed?** On `live_tiers` no arm
   moved a single metric, because the tier filter leaves 14 of 189 entries eligible
   (cas-b06c, cas-763b). Ranking work on the shipped path is capped until that changes.

## A footnote on the surviving knob

`enable_temporal` was kept — it is a real channel, hardcoded on at
`hooks/scorer.rs:55`, and flipping it changes `HybridSearch::search`'s result set
(189 hits → 185) and its whole head. But that difference does **not** reach the @5
metric on this fixture: build_start filters to active-tier entries,
`contextual_overlap_bonus` and the high-importance-preference sort dominate the
survivors, and the top-5 truncates before the temporal reordering matters. Both arms
score identically.

That is a different failure mode from the boosts and worth keeping straight. The
boosts were **discarded at the field read** — the value was never consumed. Temporal
is **consumed but washed out downstream**. The first is a defect; the second is a
consequence of the tier filter and the @5 window, and is one more reason tier
starvation (cas-763b) gates ranking work on the shipped path.
`the_temporal_arms_agree_at_5_today_and_the_harness_says_why` pins the current
equality so a future divergence arrives explained.

## What changed in the tree

- Deleted `SearchIndex::apply_boosts` and its call site.
- Deleted `SearchOptions::{boost_feedback, boost_recency, boost_importance,
  recency_half_life}` and their defaults.
- Deleted the `test_feedback_boost` unit test and the module doc example that set
  `boost_feedback: true`.
- `SearchResult::{score, boosted_score}` are retained and now both carry the
  calibrated BM25 score; `boosted_score` still has readers in
  `mcp/tools/core/search.rs`, so the field was left in place rather than renamed.
- The harness keeps a **scorer-swap A/B rig** (`ScorerConfig`,
  `ConfigurableHybridScorer`, `ProductionRunner::open_with_config`) with the three
  boost fields removed and `temporal` retained — the harness must not carry a config
  for code that no longer exists. The rig is reusable for cas-3b80 and for the
  cas-e7ae fusion decision, and is guarded by an equivalence pin: under
  `ScorerConfig::PRODUCTION` the replica must rank identically to the real
  `HybridContextScorer` on all 56 cases in both query modes.

The committed harness baseline (`cas-cli/tests/data/retrieval-eval/baseline.json`) is
**unchanged** by this deletion, and the gate is green on the final configuration.
