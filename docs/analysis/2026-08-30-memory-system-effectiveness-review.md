# Is CAS memory useful in practice? An independent audit

**Date:** 2026-08-30 · **Author:** independent Fable audit (task cas-5726) · **Code examined:** origin/main `47707f3c` · **Data:** cas-src project store (`.cas/cas.db`, read-only) as of 2026-08-30T22:30Z

## Verdict

**The supervisor's conclusion survives review in direction but not in mechanism, and several of its numbers were measuring the wrong thing.** CAS memory today works as a manually-queried archive and fails as a self-improving ambient system — but the dominant cause is not poor retrieval quality. It is that **the learning loop is structurally disconnected at four separate seams**: (1) the "Helpful Memories" injection channel is not telemetered at all, so the cited usefulness numbers never measured it; (2) outcome attribution defaults every unresolved injection to `ignored` and grants `used` only on a ≥4-char substring match, so "2 used / 593 ignored" is an artifact of instrumentation, not a measurement of value; (3) the rule lifecycle is deadlocked — only Proven rules are injected, promotion requires feedback that only a disabled-by-default reviewer path generates, therefore all 175 rules are Draft with `surface_count=0` by construction; (4) feedback and importance signals are dead in search ranking (`boost_*` flags never enabled in production). The supervisor's "≈3/10" is a fair score for *the system as a closed loop*; the archive layer alone is materially better (≈6/10), and the ambient layer is currently **unmeasurable, not proven useless**. Confidence: high on every mechanism claim (each is reproduced against code or data below); moderate on the aggregate rating, which is inherently judgment.

## Overview

| | |
|---|---|
| Question | Is CAS memory useful in practice, and was the supervisor's 3/10 review correct? |
| Verdict | Direction confirmed, mechanism corrected: loop broken by instrumentation + lifecycle deadlocks, not (provably) by retrieval quality |
| Confidence | High (mechanisms), moderate (score) |
| Scope examined | `entries`/`rules`/`retrieval_*` tables in the cas-src project DB; ambient + context-injection + feedback + rules + search code paths at `47707f3c`; 87 ambient queries, 987 logged results, 595 outcomes (2026-08-08 → 2026-08-30) |
| Not examined | Global store (`~/.cas/cas.db`), other projects' stores, cloud semantic channel behavior, session transcripts |
| Method | Read-only SQL against the live DB; two independent code-mapping passes; live MCP reproduction of the `session_id` claim |

## Scorecard: the supervisor's claims, independently checked

| # | Supervisor claim | Verdict | What the evidence actually shows |
|---|---|---|---|
| 1 | System stats: 2,657 entries; 1,510 active / 1,147 archived | **Confirmed, but misleading** | Counts reproduce exactly. "Active" means `archived=0` only. By tier, the live corpus is **26 working-tier entries**; 1,483 of the 1,510 "active" rows are `memory_tier='archive'` with `archived=0`. The two lifecycle mechanisms (tier demotion vs archive flag) disagree and neither is what retrieval filters on. |
| 2 | 175 rules, 0 proven | **Confirmed and explained** | All 175 rules Draft, `surface_count=0`, `helpful_count=0` — for every rule ever created. This is structural: only Proven rules are injected (`build_start.rs:479-483`), promotion needs `helpful_count ≥ 2` (`rules.rs:235-256`), the only production driver of helpful marks is the rule-reviewer Stop blocker, and that is **off by default** (`session_stop/mod.rs:249`, `rule_review_enabled` → `unwrap_or(false)`). Draft → Proven is unreachable in a default install. |
| 3 | retrieval_metrics: 595 ambient results, only 2 `used` | **Confirmed numerically, wrong denominator and wrong interpretation** | 595 is the count of *outcome rows*, not results: 987 results were logged, 392 (39.7%) have no outcome at all. Of the 595, 593 `ignored` / 2 `used`. Both `used` rows are `document_type='task'` (cas-e0c9, cas-c505) — **zero memory entries were ever marked used**. Only 20 entry-outcomes exist, all `ignored`. And `ignored` is written automatically at session Stop for every unresolved card (`ambient_recall.rs:2582-2597`), while `used` requires the evidence id/locator to appear as a substring in later tool input (`:2339-2347`). The metric therefore lower-bounds use so aggressively it cannot distinguish "ignored" from "read and applied without citing the id". |
| 4 | `session_id` did not narrow the aggregates | **Confirmed, root-caused** | Reproduced live: identical output with and without `session_id`. Cause: the dispatch arm drops the request (`mcp/tools/service/mod.rs:811` calls `retrieval_metrics_impl()` with no arguments; `search_context.rs:77-94` takes none; `RetrievalStore::aggregate` at `retrieval_store.rs:481-502` has no WHERE clause). The schema even mislabels the field ("Filter blame by session ID", `ops_secondary.rs:195`). Silent no-op, untested in either direction. |
| 5 | Context injected five Helpful Memories; embargo memory materially useful; release-completion preference only created after operator correction | **Consistent; the count is a hard-coded 5** | Every production caller passes `limit=5` literally (`handlers_session.rs:171,179`; `system.rs:67`); the `context_limit` config knob is dead (`config/hooks.rs:497`). The embargo entry (`2026-08-25-2`) carries `helpful_count=1`, last accessed 2026-08-30T21:57Z. The release-completion preference (`2026-08-30-8`) and rule-175 were both created after the correction — a genuine **miss** case. Critically: this injection channel writes `ContextInjectionTrace` and `surfaced_artifacts`, **not** `retrieval_queries` — the 595-row telemetry the supervisor scored never covered the five memories they were looking at. |
| 6 | rule-175 remained Draft with surfaced=0 | **Confirmed; guaranteed by claim-2 mechanism** | True for rule-175 and for all 174 others, permanently, until the promotion path is turned on. |
| 7 | Overall ≈3/10; archive good, ambient poor | **Directionally supported, mechanism corrected** | Archive/manual-lookup layer is real and better than 3 (schema, dedupe, budgeting, privacy discipline are solid). The *ambient* layer's usefulness is currently **unmeasurable** with this telemetry; what is provable is that its learning loop cannot close. Scoring "the loop": ≈3/10 is defensible. |

## Evidence table — data (all queries in `evidence-queries.txt`, artifacts root, and reproduced below in Provenance)

| Observation | Value | Source | What it proves |
|---|---|---|---|
| Entries total / archived-flag split | 2,657 / 1,510 / 1,147 | `entries` Q1 | Supervisor stats reproduce |
| Tier × archived cross-tab | working∩active = **26**; archive-tier∩active = 1,483; archived=1∩working-tier = 1,147 | Q2 | Tier and archive mechanisms disagree; "active" overstates live corpus 58× |
| Corpus composition | `context` 1,349 (avg 28.2 KB, max 130 KB); `learning` 1,238 (avg 529 B); `preference` 48; `observation` 22 | Q3, Q6 | Corpus is majority auto-captured session-context blobs, not curated memories |
| August composition | 1,215 of 1,401 new entries are auto `context` (86.7%) | Q7 | Auto-capture dominates growth |
| Feedback ever recorded | helpful on 6/2,657 entries (0.23%); harmful 0; ever-accessed 175 (6.6%) | Q4, Q17 | Manual feedback loop essentially unused |
| Embeddings | `pending_embedding=1` for all 2,657 | Q5 | Local semantic pipeline inactive; local ranking is lexical + heuristics |
| Rules | 175 draft / 0 proven / `surface_count` sum 0 / helpful sum 0 | Q8 | Rule flywheel has never turned once |
| Retrieval queries | 119 total: 87 ambient (70 transition + 17 session_start), 32 explicit-search | Q9 | Telemetry window 2026-08-08 → 2026-08-30 |
| Results vs outcomes | 987 results; 595 outcomes (593 ignored, 2 used); 392 results no outcome | Q10-Q11, Q14 | 39.7% of injections never resolved; supervisor's 595 = outcomes, not results |
| Outcomes by type | entry: 20 ignored, 0 used; history: 465 ignored; task: 35 ignored + **2 used** | Q12-Q13 | The only "used" signals are task ids matched by substring; entries scored zero — but see attribution mechanism |
| Distinct entries ever ambiently surfaced | 24 (5 context, 13 learning, 6 preference); 20 of 24 archive-tier | Q15-Q16 | Ambient path touches ~0.9% of corpus; tier ignored in selection |
| `retrieval_metrics` with vs without `session_id` | byte-identical JSON | live MCP call, this session | Filter silently ignored |
| This session's own session-start bundle | 3 entries injected (2 archive-tier release-discipline preferences, 1 codemap note); 0 relevant to this audit task | Q in provenance; `qry-ambient-38942f8a...` | n=1 live probe of ambient relevance for a worker session |

## Evidence table — code (all paths relative to repo root at `47707f3c`)

| Mechanism | Location | Finding |
|---|---|---|
| "Helpful Memories" builder | `crates/cas-core/src/hooks/context/build_start.rs:686-809` | Selects from `store.list()` (`archived=0 LIMIT 10000`), filters only type/expiry — **`memory_tier` never consulted**; Cold/Archive-tier entries fully eligible |
| Injection count | `cas-cli/src/hooks/handlers/handlers_session.rs:171,179` etc. | `5` hard-coded at every call site; `hooks.context_limit` config dead (`cas-cli/src/config/hooks.rs:497`) |
| Scoring formula | `crates/cas-core/src/hooks/context/mod.rs:144-189`; blended 70/30 with hybrid in `cas-cli/src/hooks/scorer.rs:145-157` | type-weight (Preference 2.5, Learning 1.5, Context 1.3, Observation 0.3) × feedback × age-decay (floor 0.5 at 25 days) × importance × stability × access boosts |
| Feedback boosts in search | `cas-cli/src/hybrid_search/mod.rs:262-264` | `boost_feedback/recency/importance` default false and **no production caller sets them true** — helpful/harmful/importance never affect BM25 ranking; only the 30% basic-scorer blend sees them |
| Telemetry writers | ambient packet: `cas-cli/src/ambient_recall.rs:2259-2294`; explicit search (only with `provenance_version=1`): `mcp/tools/core/search.rs:612-648` | The Helpful Memories channel writes **no** `retrieval_queries` rows — the audited metric never measured it |
| Automatic `used` | `ambient_recall.rs:2339-2347`, wired at `handlers_middle/post_tool.rs:36` | Substring match of evidence id/locator (≥4 chars) in lowercased tool name+input |
| Automatic `ignored` | `ambient_recall.rs:2582-2597`, wired at `session_stop/stop_flow.rs:699` | Every still-unresolved card marked ignored at un-blocked Stop; `None` activity = blanket default |
| Outcome → ranking feedback | `ambient_recall.rs:2178-2191, 2196-2257` | `(0.35·used + helpful − 0.25·ignored − 0.75·corrected − 1.25·harmful)/(n+4)·0.2`, clamped [−0.20, +0.15] — auto-ignored entries acquire a *negative* ranking prior from the attribution default |
| Rule injection filter | `crates/cas-core/src/hooks/context/build_start.rs:479-483` | `status == Proven` required; Draft rules never injected, never synced (`sync/mod.rs:118`), only softly recallable via ambient (`ambient_recall.rs:1158-1171`) |
| Rule promotion | `cas-cli/src/mcp/tools/core/rules.rs:235-256`; threshold floor 2 | Only driver is `cas_rule_helpful`; intended caller is rule-reviewer, summoned by a Stop blocker gated on `rule_review_enabled` default **false** (`session_stop/mod.rs:249`) |
| `surface_count` increments | `build_start.rs:46-57, 510, 589`; `surfaced_artifact_store.rs:136-143` | Correctly implemented — but only reachable for injected (= Proven) rules; ambient rule cards bypass it. Two unreconciled surfacing ledgers |
| `retrieval_metrics` param handling | `mcp/tools/service/mod.rs:811` → `search_context.rs:77-94` → `retrieval_store.rs:481-502` | Request dropped at dispatch; no filter parameters exist; `aggregate_for_result` (`:331`) proves the scoped-read pattern exists but no session variant was written |
| Mid-session ambient recall | `handlers_middle/prompt_capture.rs:72,86` | Factory-agents-only; ordinary sessions get ambient recall once, at SessionStart |
| Decay/tier job | `cas-cli/src/daemon/decay.rs:22-108` | Demotes tiers that retrieval then ignores (see build_start finding); `auto_prune` → `archived=1` is the only transition retrieval respects |
| Hard-policy boundary | `handlers_events/pre_tool.rs` (deny/allow); `crates/cas-types/src/rule.rs:247-311` | Enforcement is hardcoded Rust; the only data-driven enforcement is Proven-rule auto-approval of safe read-only tools. No memory or rule body can create a deny. Rules' `hook_command` field is stored but never executed (dead capability) |

## Reasoning chain

1. **The supervisor's numbers are real** — every count reproduced exactly against the live store (claims 1–3, 6). Nothing was fabricated or misread at the data layer.
2. **But the headline metric measured a channel that doesn't carry the product.** The five Helpful Memories the supervisor evaluated come from `build_start.rs`; the 595-outcome telemetry comes exclusively from the separate ambient-recall packet and provenance-enabled explicit searches. Judging "ambient memory usefulness" from `retrieval_metrics` is judging the wrong pipe.
3. **Within the telemetered pipe, the outcome labels are defaults, not observations.** `ignored` is assigned en masse at Stop; `used` needs a literal id/locator substring in tool input. An agent that reads an injected memory and follows its advice without re-typing its id is recorded as `ignored`. Therefore "0 of 20 entry-results used" is compatible with both "useless" and "quietly useful" — the instrument cannot tell. This resolves the actual-use-vs-attribution question the task posed: **the ambiguity is real and currently unresolvable from stored telemetry; the data lower-bounds use only.**
4. **The one place outcomes do have teeth is quietly harmful:** auto-`ignored` feeds a negative ranking adjustment (−0.25 weight) and rule promotion counts `used+helpful` — so the attribution defaults actively depress ranking of injected-but-unmatched items and block retrieval-evidence promotion.
5. **The rule layer cannot learn in a default install.** Injection requires Proven; promotion requires feedback; feedback generation is disabled by default; ambient surfacing of Draft rules doesn't count toward `surface_count`. Four locks on one door. The observed 175/0/0 is not underperformance, it is the fixed point of the design as shipped.
6. **The corpus composition undermines ambient precision independently of ranking.** 86.7% of August growth is 28 KB auto-context blobs; the scorer's Context type-weight (1.3) is close to Learning's (1.5), the semantic channel is inactive locally (all 2,657 entries pending embedding), and tier demotion — the mechanism meant to age noise out — is invisible to selection. The 26-entry curated working set the operator actually relies on competes with 1,484 stale-but-"active" rows.
7. **What memory demonstrably did this cycle:** helped once (Slack-embargo preference surfaced and honored; also the only working-tier preference with fresh access + helpful mark); missed once (release-completion requirement had to come from operator correction — the prior handoff entries contained adjacent context but no retrievable statement of the requirement); and was unaffected for the bulk (465 history-card injections all auto-ignored; this audit session's own 3-entry bundle was 0-relevant to its task). One hit, one miss, high chaff volume — consistent with "archive good, ambient weak", but on an n too small to rate retrieval quality; what it does rate is the absence of any mechanism that would have *told us* the answer at scale.
8. **Therefore:** overturn the supervisor's implicit mechanism ("retrieval is poor"), sustain the direction ("the system doesn't function as autonomous memory"), and re-target the fix program at instrumentation and lifecycle seams before touching ranking math — improving a ranker you cannot measure is unfalsifiable work.

## What would falsify this

- **The channel-coverage claim** dies if `build_context_with_stores` or its CLI wrapper is shown writing `retrieval_queries` rows for Helpful Memories items (it writes `ContextInjectionTrace` + `surfaced_artifacts` only; grep `record_ambient_query` callers).
- **The attribution claim** dies if any code path marks `used` other than the substring matcher and explicit `retrieval_feedback` (both writers enumerated at `retrieval_store.rs` callers), or if `ignored` requires evidence of non-use.
- **The rule-deadlock claim** dies if any production path promotes a rule without `cas_rule_helpful`, or if `rule_review_enabled` defaults true anywhere reachable, or if a Draft rule is shown in a SessionStart "Active Rules" block.
- **The dead-boost claim** dies if a production (non-test) call site sets any `boost_*` flag true.
- **The tier-invisibility claim** dies if any retrieval candidate query filters on `memory_tier` (only `archived=0` appears in `store_list`).
- **The overall verdict** softens if a labeled evaluation (below) shows ≥50% of injected memories rated relevant by the receiving agent — that would restore "retrieval is fine, only measurement is broken" and raise the score.

## Prioritized improvement program

Owner legend: **S** = supervisor-filed factory task (Rust change in this repo), **O** = operator/config decision.

| P | Change | Seam (file:line) | Acceptance metric | Tests/evals | Risk |
|---|---|---|---|---|---|
| 1 | **Telemeter the real injection channel.** Log every Helpful-Memories item as a `retrieval_queries`/`retrieval_query_results` row (family `context_session_start`, distinct policy tag), or route the section through ambient recall. (S) | `cas-cli/src/hooks/context.rs:150-229` (where `ContextInjectionTrace` is already assembled); reuse `record_ambient_query` shape from `ambient_recall.rs:2259` | 100% of injected memory items appear in telemetry; denominator documented in `retrieval_metrics` output | Unit: injection writes N rows for N items; parity with trace. Update `search_tools.rs` metrics flow test | Low; write-path only. Watch hook latency budget (5s timeout) |
| 2 | **Make outcomes honest.** Split `ignored` into `unresolved` (Stop default) vs true negatives; stop feeding `unresolved` into `outcome_adjustment` and promotion denominators; add a positive signal cheaper than substring — e.g. count `memory action=get/list` pulls of an injected id, which the packet already invites ("bodies=tool-pull-only"). (S) | `ambient_recall.rs:2582-2597` (finalize), `:2178-2191` (adjustment), `retrieval_store.rs:24-66` CHECK constraint + migration | `usefulness_rate` computed only over resolved outcomes; body-pull events recorded as `used`; no ranking penalty from defaults | Extend `automatic_hook_feedback_populates_metrics_with_plausible_attribution` (`ambient_recall.rs:3419`); new test: body-pull → used | Medium: outcome enum change touches CHECK constraint; keep `INSERT OR IGNORE` idempotence |
| 3 | **Unjam the rule flywheel.** (a) Decide the promotion driver: enable `rule_review_enabled` by default *or* schedule rule-reviewer from daemon maintenance; (b) count ambient rule surfacing into `surface_count`/`surfaced_artifacts` so the reviewer sees real exposure; (c) reconcile the two surfacing ledgers. (S + O for the default) | `session_stop/mod.rs:249`; `ambient_recall.rs:1158-1171` rule surface; `surfaced_artifact_store.rs:136-143` | ≥1 rule reaches Proven within 20 sessions of enablement; `sum(surface_count) > 0`; one ledger or a documented join | `rule_tools.rs` promotion suite already exists; add ambient-surface→counter test | Reviewer cost per Stop; mitigate with threshold + worker exemption (already present) |
| 4 | **Fix `retrieval_metrics` scoping.** Honor `session_id` (derive `session_hash`, add WHERE) and **error on unsupported filters** instead of ignoring them; fix the `session_id` schema description. (S) | `mcp/tools/service/mod.rs:811`; `search_context.rs:77-94`; add `aggregate_for_session` beside `retrieval_store.rs:331`; `ops_secondary.rs:195,198` | Filtered call returns strict subset; unsupported param → explicit error | New test beside `search_tools.rs:501` asserting narrowing (currently untested in both directions) | Low |
| 5 | **Corpus hygiene + tier truth.** Exclude `type='context'` blobs from Helpful-Memories candidacy (or cap preview + require feedback>0); make selection respect `memory_tier`; reconcile `archived` flag vs tier semantics in `system stats` so "active" means live. (S) | `build_start.rs:704-709` filter; `store_entry_crud.rs:388-403`; stats reporter | Working-set candidates = working/in-context tiers; stats report tier truth; injected-item relevance (P7 harness) improves | Add both-direction tier-eligibility tests (none exist today); decay-job tests already cover transitions | Medium: could hide a genuinely useful archived entry; mitigate with explicit `memory action=search` escape hatch (already exists) |
| 6 | **Decide the dead controls.** Either enable feedback/recency/importance boosts in production search defaults or delete `apply_boosts`; wire `hooks.context_limit` or delete it; delete diverged duplicates (`crates/cas-core/src/search/{scorer,query_ops}.rs`, `crates/cas-core/src/migration/`), the broken `store_list_decayable` tier string (`'in_context'` vs `'in-context'`, `store_entry_crud.rs:401`), and the never-executed `hook_command`. (S) | listed inline | No config knob that silently does nothing; one implementation per subsystem | `cargo check` + existing suites; add a config-honored test for `context_limit` | Low-medium (ranking change if boosts enabled — gate behind P7 harness first) |
| 7 | **Evaluation harness** (prerequisite for any ranking work) — see below. (S) | new `cas-cli/tests/` + daemon job | harness exists, baseline captured, gate wired | — | — |

## Proposed evaluation harness

Three layers, cheapest first; all offline-runnable against a store snapshot:

1. **Deterministic replay (exists, extend).** `cas-cli/tests/retrieval_parity_test.rs` already replays captured queries with rank-drop tolerance. Extend with a *labeled* fixture set: ~50 (prompt-context → expected-relevant entry ids) pairs mined from real session-start bundles and judged once by hand. Metric: precision@5 and recall@5 for the Helpful-Memories selector and the ambient packet, tracked per commit. Gate: no silent regression >10% relative.
2. **Injected-relevance sampling (new, cheap).** After P1, a weekly daemon job samples N injected bundles and asks the *receiving* agent (or a scheduled judge run) for a 0/1 relevance label per item, written as explicit `retrieval_feedback`. Metric: rolling injected-precision; this is the number that would falsify or confirm "ambient retrieval is poor" — which today is unknowable. Target to beat: 50%.
3. **Outcome-funnel dashboard (new).** From honest outcomes (P2): injections → resolved → used/body-pulled → helpful, per document_type and per query_family, with the denominator (results, not outcomes) stated. Publish in `system stats` so the next audit doesn't re-derive it by hand.

## What should be enforced outside memory

The audit confirms the codebase already draws the right hard/soft line — enforcement lives in Rust PreToolUse guards and Stop blockers, and **no memory or rule body can create a deny** (`pre_tool.rs`; auto-approval capped to read-only tools, `rule.rs:247-311`). Keep it that way, and move three things across the line rather than hoping recall catches them:

- **Standing operator prohibitions with dates** (Slack embargo, release-completion contract): these are exactly the "must not miss" class. Encode as Proven rules with `valid_until`, or as explicit hook checks; ambient recall as backup, never primary.
- **Session handoffs**: the operator's real continuity mechanism is the harness `MEMORY.md` flat file, which is versioned, always-loaded, and curated — CAS `context` blobs duplicate it poorly at 28 KB each. Stop auto-competing with it; capture less, link more.
- **Anything with a deadline or a count** (worker caps, embargo windows): memory recall is probabilistic; policy must not be.

## Threats to validity

- **Single project, single store.** Only the cas-src project DB was examined; global-store and other-project dynamics may differ. The supervisor's numbers matched this store exactly, so the review and this audit at least examined the same data.
- **Small live-relevance sample.** The helped/missed/unaffected assessment rests on the supervisor's attested session plus one live probe (this session's 3-item bundle, n=1). It is presented as illustration, not measurement; the measurement gap is itself finding #1.
- **Supervisor review known only via the task description.** The full text from session cas-src-vivid-wolf-17 was not persisted as an artifact; claims were audited as quoted in cas-5726. If the original review contained additional caveats, the scorecard's "wrong interpretation" cells may be unfair to it.
- **Telemetry window is short and sparse** (2026-08-08→08-30, 119 queries, 11 sessions with outcomes, none after 08-27 except un-outcomed queries). All rates carry wide intervals; that is why no precision claims are made from them.
- **`last_accessed`/`access_count` conflate admin listing with useful recall** — the 6.6% ever-accessed figure is an upper bound on deliberate lookup.
- **This session ran under the audited system** (its hooks injected ambient context into this audit), a mild observer effect; no memory writes were made to the store during evidence collection beyond the telemetry rows the system itself logs for any session.

## Provenance

- **Code:** repo `Richards-LLC/cassy` worktree at origin/main `47707f3c` (merge of release v3.7.1). All file:line citations refer to this tree.
- **Data:** `/home/pippenz/Petrastella/cas-src/.cas/cas.db` opened read-only (`file:...?mode=ro`) via sqlite3, 2026-08-30 22:26–22:45 UTC. Full query set + raw outputs: `/home/pippenz/.cas/artifacts/cas-5726/evidence-queries.txt` (Q1–Q17 referenced above, in file order).
- **Live reproduction:** `mcp__cas__search action=retrieval_metrics` called twice this session (with and without `session_id=sha256:3a65...da1`); outputs byte-identical.
- **Code mapping:** two independent Explore passes over `cas-cli/src/` and `crates/` (memory/context/ambient/decay; feedback/rules/search/policy), findings cross-checked against this auditor's own reads of the cited lines.
- **Key row-level evidence:** used-outcome rows (`cas-e0c9`, `cas-c505`, both `document_type='task'`); session-start query `qry-ambient-38942f8ad9e641aea2fb7f2a63a93b7c` (this session, 3 entry results); helpful-marked entries listed in Q17.
- This document: `docs/analysis/2026-08-30-memory-system-effectiveness-review.md`; HTML twin beside it, generated from this markdown in the same commit.
