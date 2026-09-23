# Model lane rubric refresh — 2026-09-23

Audience: operator. Type: decision brief. Window: 2026-09-06 00:00Z through 2026-09-23 23:59Z.
This is a measurement and placement brief only; it does not edit `lane-registry.toml`, routing code,
or Rust.

## Recommendation

Keep the 2026-09-06 lane intent as a receipt-gated provisional policy: Luna/xhigh remains standard,
Haiku remains light-only, Fable/high remains the explicit taste/supervisor target, and Astra/high stays
bounded heavy work only. Do not call heavy validated: the refresh contains 47 closed Astra/high tasks
but **zero Sol/high rows**, so the promised first-five paired comparison has not happened. Make
Claude Opus 5.5/high the next fallback candidate to measure, not an unreviewed registry edit.

The strongest measured fact is a warning, not a winner: Luna/xhigh has 315 delivered tasks at a
median $0.89 per delivered task and 23.81% send-backs; Astra/high has 47 at $16.71 and 29.79%.
Those are different workloads, and the absence of Sol/high means they cannot decide the heavy lane.

### Decision ledger

| Option | Cost signal | Risk / evidence | Reversibility | Verdict |
| --- | ---: | --- | --- | --- |
| Keep the 2026-09-06 placements, but receipt-gate Astra/high and require five Astra/Sol pairs | Luna $0.89/task; Astra $16.71/task | Preserves the intelligence hypothesis without pretending the control exists; supervisor role and attribution remain open | Policy-only; reversible at the next lane review | **Recommended** |
| Replace heavy with Luna/xhigh on observed send-back rate | Luna 23.81% vs Astra 29.79% | Cheapest, largest sample, but changes the intelligence-risk policy without a matched brief | Easy, but a safety-sensitive miss is costly | Hold as rescue, not primary |
| Promote Opus 5.5/high as the cross-lane fallback now | $7.66/task; 20 delivered; 5.00% send-backs | Promising current model, but no historical paired comparison and no first-push markers | Easy recipe change | Measure first |

### Recommended placement

This table is the decision surface. “Number” is measured in the companion summary CSV; the model
price basis is the current vendor list price, not subscription spend.

| Lane | Primary / effort | Fallback | Number that places it | Sample / confidence |
| --- | --- | --- | --- | --- |
| standard | Codex Luna / xhigh | Claude Opus 5.5 / high candidate | 315 delivered; 23.81% send-backs; $0.89/task; 4 urgent stops | n=315; broadest named sample |
| light | Claude Haiku 4.5 / numeric thinking budget to be specified | Luna / xhigh for code-touching work | 17 delivered; 29.41% send-backs; $1.65/task | n=17; bounded chores only |
| taste | Claude Fable 5.1 / high, explicit | Opus 5.5 / high candidate | 14 delivered; 42.86% send-backs; $113.16/task; medium is 33 delivered and 18.18%, but not a paired test | n=14 high; provisional |
| supervisor | Claude Fable 5.1 / high, explicit | Opus 5.5 / high candidate | No role-attributed supervisor sample in this extract; registry still says medium | unverified until role attribution exists |
| heavy | Codex Astra / high, bounded and receipt-gated | Codex Sol / high | 47 delivered; 29.79% send-backs; $16.71/task; **0 Sol/high rows** | n=47 Astra; no control |

## Decision context

The operator must decide whether the 2026-09-06 placement still deserves to be the working rubric
while the next controlled measurement is collected. This refresh answers what changed in the
2026-09-06 → 2026-09-23 window, which action items are complete, and where the live registry still
differs from the measured recommendation. It is not a request to change policy in this commit.

## Measured model × effort

The summary CSV has one row per model × effort. Sessions are distinct session IDs after the extractor's
`(session_id, task_id)` de-duplication; delivered tasks are distinct closed task IDs. Send-backs and
urgent stops are deduplicated task-note counts. Token and tool columns are medians per delivered task;
`in / cached / out` are tokens, and “push” is median minutes to the first logged push. A blank push is
missing log evidence, not zero. Costs sum each priced session once and divide by priced delivered tasks.

| Model / effort | Sessions | Delivered | Send-backs | Urgent | Median tokens: in / cached / out | MCP calls | Push min | Current list $ / task |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Luna / xhigh | 368 | 315 | 75 (23.81%) | 4 | 17,819,040 / 17,350,144 / 50,425 | 135.50 | 32.35 | **$0.89** |
| Haiku 4.5 / low | 17 | 17 | 5 (29.41%) | 0 | 1,177 / 8,469,252 / 48,179 | 57.00 | 27.29 | $1.65 |
| Opus 5.5 / high | 12 | 20 | 1 (5.00%) | 0 | 17,899,513 / 74,888 / 74,888 | 70.50 | — | $7.66 |
| Opus 5.5 / medium | 2 | 9 | 2 (22.22%) | 0 | 1,568 / 21,619,521 / 94,610 | 50.80 | 169.56 | $10.96 |
| Astra / medium | 3 | 3 | 0 (0.00%) | 0 | 2,207,571 / 2,138,112 / 6,695 | 39.00 | 6.22 | $3.13 |
| Astra / high | 54 | 47 | 14 (29.79%) | 0 | 9,029,939 / 8,902,976 / 29,262 | 79.50 | 23.51 | $16.71 |
| Astra / xhigh | 2 | 2 | 0 (0.00%) | 0 | 12,169,291 / 11,933,248 / 57,677 | 91.00 | 84.79 | $17.18 |
| Opus 5 / high | 71 | 57 | 9 (15.79%) | 0 | 538 / 46,022,521 / 213,700 | 175.00 | 27.57 | $76.83 |
| Opus 5 / max | 2 | 2 | 1 (50.00%) | 0 | 4,303 / 241,536,769 / 690,112 | 367.00 | 50.11 | $174.74 |
| Fable 5.1 / medium | 36 | 33 | 6 (18.18%) | 0 | 3,494 / 16,761,367 / 165,453 | 85.00 | 17.29 | $42.84 |
| Fable 5.1 / high | 17 | 14 | 6 (42.86%) | 0 | 7,641 / 84,980,790 / 634,453 | 191.00 | 41.67 | $113.16 |
| Sonnet 5 / high | 3 | 2 | 0 (0.00%) | 0 | 2,711 / 591,193,668 / 661,990 | 785.50 | 50.59 | $138.21 |
| Sol / high | 0 | 0 | — | 0 | — | — | — | — |
| Terra / high | 0 | 0 | — | 0 | — | — | — | — |

Current list-price inputs are the official [OpenAI pricing table](https://developers.openai.com/api/docs/pricing)
and [Anthropic pricing table](https://docs.anthropic.com/en/docs/about-claude/pricing), retrieved for this
refresh. OpenAI’s table lists Astra at $10/$1/$50 and the GPT-5.6 Sol/Luna family at the rates used in
the CSV; Anthropic lists Fable 5.1 at $10/$0.25/$20/$50, Opus 5.5 at $4/$0.20/$8/$20, Sonnet 5 at
$2/$0.20/$4/$10, and Haiku 4.5 at $1/$0.10/$2/$5 per million input/cache-read/cache-write-1h/output
tokens. Cached input is a subset for Codex; the CSV’s `cost_usd_current` column bills it once.

### Attribution gap

The refresh has 1,643 evidence rows across 13 projects. 958 rows (58.31%) have no model, and the
deduplicated session set has 530 of 1,137 sessions (46.61%) with no model at all. Those rows still
carry delivery and note counts, so they remain visible rather than being dropped; they cannot support
a model placement. The aliases `opus`, `<synthetic>`, and other unpriced rows are likewise retained in
the CSV and excluded from the named-lane recommendation.

## Status of the 2026-09-06 action items

| Action item | Status | Evidence in this window |
| --- | --- | --- |
| Measure the first five Astra/high vs Sol/high deliveries on matched briefs | **Open — not measured** | 54 Astra/high sessions / 47 delivered tasks; **0 Sol/high rows**; no matched pairs can be formed |
| Give Haiku a numeric `thinking_budget` instead of named `low` | **Open — not implemented** | 17 Haiku/low deliveries, 5 send-backs; `recipes.claude_haiku` still exposes only `default_effort = "low"` |
| Set the supervisor recipe to high | **Open — registry still medium** | `recipes.claude_fable.default_effort = "medium"`; Fable high/medium worker rows exist, but transcript rows do not prove supervisor role |
| Fix model attribution and report the no-model share | **Partial — extractor improved, source gap remains** | All 11 configured Codex/Claude homes scanned; model/effort recovered from Codex `turn_context`; 46.61% of unique sessions still have no model |

## Drift from the 2026-09-06 recommendation

| Recommendation on 2026-09-06 | Registry on 2026-09-23 | Drift and consequence |
| --- | --- | --- |
| Standard = Luna/xhigh | `[lanes.standard]` still candidates Luna then `claude_opus` | Primary matches; fallback still pins Opus 5/high rather than the available Opus 5.5 |
| Light = Haiku with a numeric thinking budget | `claude_haiku.default_effort = "low"` | Named effort has no numeric thinking-budget meaning; 17 current deliveries do not validate a code lane |
| Taste = Fable/high | `claude_fable.default_effort = "medium"`; taste candidate is Fable | **Drift:** the live recipe is medium, and current rows cannot prove which effort belongs to taste work |
| Supervisor = Fable/high | `claude_fable.default_effort = "medium"`; supervisor candidate is Fable | **Drift:** the action item remains open; the data lacks role attribution for a clean supervisor result |
| Heavy = Astra/high, Sol/high fallback | `[lanes.heavy]` uses `codex_astra_high` and falls back to `codex_sol` | Placement matches; there are no Sol/high observations in this window, so the fallback is unmeasured |
| Avoid unpinned Astra/medium | `recipes.codex_astra.default_effort = "medium"` remains active | **Drift:** an explicit heavy recipe is safe, but an unpinned Astra spawn still inherits medium |
| Use the newest priced Claude fallback | `recipes.claude_opus.model = "claude-opus-5"` | **Drift:** Opus 5.5 and Sonnet 5 are available at current list prices; Opus 5.5/high measured $7.66/task vs Opus 5/high $76.83/task, but the comparison is not controlled |

## Why this recommendation

The recommendation preserves the useful part of the prior decision while making the evidence boundary
explicit. Luna is the only named lane with hundreds of delivered tasks. Astra/high is now large enough
to run a paired trial, but its 29.79% send-back rate and $16.71/task do not beat Luna on the observed
workload, and there is no Sol control. Opus 5.5/high is a credible fallback candidate because its
20-task sample is 5.00% send-backs at $7.66/task, but its rows are concentrated in two projects and
have no first-push markers; that is a measurement lead, not permission to route silently.

## What we give up

We give up a possibly cheaper or more reliable heavy placement today. Promoting Luna would optimize
the observed send-back rate and cost but discard the unresolved intelligence-risk hypothesis; promoting
Opus 5.5 would use an attractive sample without a matched baseline. Holding Astra as bounded and
receipt-gated keeps both claims testable.

## Reversal cost

The recommendation is reversible at the policy layer: a later decision can change the recipe or fallback
without Rust changes. The expensive part is not the edit; it is collecting five matched Astra/high and
Sol/high briefs, recording role-attributed supervisor sessions, and backfilling `worker_spec` model
metadata so the next refresh can distinguish a real lane effect from task mix.

## Open questions

1. Which five bounded briefs will be run on both Astra/high and Sol/high, with identical acceptance criteria?
2. What numeric Haiku thinking budget is acceptable for light chores, and what marker test bounds it?
3. How will factory sessions record supervisor role and requested model/effort so task rows are attributable?
4. Should Opus 5.5/high replace Opus 5/high as the fallback after a short receipt-gated trial?

## Method

The existing extractor was run once over the host root so all configured homes and projects were in the
same measurement. The inclusive date filter uses `session_first_at`, then `spawn_at`, lease acquisition,
or task creation as the row date. The extractor scans 45 CAS databases, 3 Codex homes and 8 Claude homes;
its source receipt reports 7,193 JSONL files, 3,029 factory transcripts, and 2,580 DB joins. The evidence
CSV is the 1,643-row filtered output. The summary CSV applies the extractor’s scorecard definitions:
`(session_id, task_id)` de-duplication, distinct closed task IDs, task-note send-back/urgent counts,
medians per delivered task, and one price calculation per session. Missing values are blank, not zero.

Commands and source paths:

```text
python3 scripts/factory-model-history.py --root /home/pippenz --date 2026-09-23
docs/factory/2026-09-23-model-lane-rubric-refresh.csv
docs/factory/data/factory-model-history-2026-09-23-horizon.csv
docs/factory/data/factory-model-history-2026-09-23-sources.md
```

## Provenance

- Registry inspected: `crates/cas-factory/policy/lane-registry.toml`, especially lines 19–24, 37–55,
  57–77, and 109–131.
- Extractor behavior: `scripts/factory-model-history.py`, all-home discovery at lines 474–478, Codex
  `turn_context` model/effort recovery at lines 383–390, bounded transcript join at lines 825–917, and
  cache-safe cost billing at lines 953–978.
- Row-level evidence: [`2026-09-23-model-lane-rubric-refresh.csv`](2026-09-23-model-lane-rubric-refresh.csv)
  (summary) and [`factory-model-history-2026-09-23-horizon.csv`](data/factory-model-history-2026-09-23-horizon.csv)
  (filtered session/task rows).
- Extraction source receipt: [`factory-model-history-2026-09-23-sources.md`](data/factory-model-history-2026-09-23-sources.md).
- Pricing sources: [OpenAI](https://developers.openai.com/api/docs/pricing) and
  [Anthropic](https://docs.anthropic.com/en/docs/about-claude/pricing), retrieved 2026-09-23.
- No lane registry, routing, or Rust files were changed for this decision brief.
