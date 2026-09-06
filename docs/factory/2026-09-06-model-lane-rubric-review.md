# Model lane rubric review — 2026-09-06

Audience: operator (Daniel). Type: decision brief. Author: supervisor session golden-panda-80.
Evidence window: 2026-09-05 16:20Z – 2026-09-06 00:10Z (the cas-80b6 rescue, releases 3.17.1 and 3.17.2).

## Verdict

The rubric routes by reputation, not by measurement. In the one window where we have hard numbers,
the lane that carried the work (Codex Luna at xhigh, "standard") is absent from the risk-bearing
lane ("heavy"), the model with the only recorded stall (Codex Astra) is being promoted into that
lane, and the two judgment lanes now fall back silently to the model they replaced. Nothing in the
rubric names an effort rationale, a cost ceiling, or a promotion rule. Three numbers would fix that,
and one of them already ships in 3.17.2.

## The rubric as directed (2026-09-06 00:10Z, task cas-255e in flight)

| Lane | Primary | Effort | Fallback | Intended use |
|---|---|---|---|---|
| light | Claude Haiku 4.5 | low | Codex Luna / xhigh | mechanical chores |
| standard | Codex GPT-5.6 Luna | xhigh | Claude Opus 5 / high | ordinary implementation |
| taste | Claude Fable 5.1 | medium | Claude Opus 5 / high | public surfaces, prompts, docs, judgment |
| supervisor | Claude Fable 5.1 | medium | Claude Opus 5 / high | factory coordinator |
| heavy | Codex GPT-6 Astra | high | Codex GPT-5.6 Sol / high | implementation with safety risk |

Explicit-only recipes: Codex Sol (former heavy), Codex Terra (suspended 2026-08-27), Qwen 3.8 Max via
OpenCode (receipt-gated). Source: `crates/cas-factory/policy/lane-registry.toml` at epic tip
736bb1fe plus the cas-255e directive.

## What the evidence window shows

Deliveries reviewed by the supervisor between 16:20Z and 00:10Z, by the lane that produced them.
"Send-back" means a review rejection or CI red that required a corrective commit before merge.

| Lane (model / effort) | Workers | Deliveries | Send-backs | CI reds | Merged |
|---|---|---|---|---|---|
| standard (Luna / xhigh) | 8 | 19 | 5 | 1 | 19 |
| heavy (Sol / high) | 1 | 1 | 1 | 0 | 1 (after continuation on Luna) |
| taste (Fable / medium) | 0 as worker; 1 as supervisor | — | — | — | — |
| light (Haiku / low) | 0 | 0 | 0 | 0 | 0 |
| Astra (any) | 0 as worker | 0 | 0 | 0 | 0 |

Send-back detail (standard): cas-62ca ×2 (doctor snapshot row missed; managed-block line budget),
cas-d05f ×2 (fixture escaped the checkout; then a helper contract change that generalised the same
defect), cas-1e85 ×1 (release-note "Was" wording). Send-back detail (heavy): cas-c674 deleted the
supervisor skill's Operating flow section and conflicted with the epic; a Luna worker finished it.

Supervisor stall on record: the previous supervisor held finished workers with green proofs from
about 06:00Z to 15:10Z on 2026-09-05 (nine hours of actionable idle) — the operator attributes this
behaviour to Astra. Source: cas-20a3 note 15:10Z; operator statement 17:27Z.

Release latency in the same window: 3.17.1 from rescue start to published 4 h 57 min, including two
merge-queue failures caused by a test from an earlier lane; 3.17.2 from "cut it now" to published
77 min with one gate and one queue run.

## Model intelligence, cost and efficiency

Added 2026-09-06 by worker vivid-kestrel-88 (task cas-3372) at the operator's request: "it is a dance
between token efficiency, token cost, and intelligence, and even tool calling". Three parts: what the
vendors and third parties publish per model and effort (A), what this host's own sessions measured per
delivery over the whole retained horizon 2026-08-20 → 2026-09-06 (B, full table in
`2026-09-06-model-lane-history.md` + `.csv`; cas-src only, all three Codex homes and all three Claude homes on the host), and the synthesis (C). Every external number carries its
URL; all were retrieved 2026-09-06. "V" = vendor-published, "3P" = third party (Artificial Analysis =
AA, Epoch, goml.io, vellum.ai, jessemoraga.com). Unknown means not found at the URLs checked, not zero.

### A. What is published

**Effort vocabulary.** No two providers expose the same dial, and none exposes `minimal` on the models we
route. Mapping to Cassy's minimal/low/medium/high/xhigh:

| Provider / model | Exposed levels | Default | Our `minimal` | Our `xhigh` | Source |
|---|---|---|---|---|---|
| OpenAI GPT-5.6 Luna / Sol / Terra (API `reasoning.effort`) | none, low, medium, high, xhigh, max (+ `reasoning.mode` standard/pro; Codex runtime adds `ultra` for Sol) | medium | no equivalent (`none` is closest) | xhigh | https://developers.openai.com/api/docs/guides/reasoning ; https://github.com/openai/codex/issues/33233 |
| OpenAI GPT-6 Astra | low, medium, high, xhigh, max (no `none`) | medium | none | xhigh | https://developers.openai.com/api/docs/models/gpt-6-astra |
| Anthropic Fable 5.1 / Opus 5 / Sonnet 5 (`output_config.effort`; Claude Code `--effort`) | low, medium, high, xhigh, max | high (Claude Code and API); medium in Claude.ai/Cowork | none (lowest is low) | xhigh | https://docs.anthropic.com/en/docs/build-with-claude/effort ; https://www.anthropic.com/claude-fable-and-mythos-5-1 |
| Anthropic Haiku 4.5 | no effort parameter; manual `thinking.budget_tokens` only | — | thinking off | 128K budget | https://platform.claude.com/docs/en/models/haiku-4-5/overview |
| xAI Grok 4.5 (`reasoning_effort`) | low, medium, high; reasoning cannot be disabled | high | none | none (xhigh exists only on grok-4.20-multi-agent) | https://docs.x.ai/developers/model-capabilities/text/reasoning |
| Alibaba Qwen 3.8 Max | `enable_thinking` on/off + numeric `thinking_budget` (≤262,144); no named levels | thinking on | thinking off | budget = max | https://docs.modelstudio.console.alibabacloud.com/en/model-studio/deep-thinking |

Codex's `model_context_window` of 258,400 on this host is the Codex default 272K profile minus headroom,
not the API window: the API window for all GPT-5.6 models and Astra is 1,050,000 and Codex can be raised
to 872,000 (https://github.com/openai/codex/issues/39144, https://github.com/openai/codex/pull/39102).

**Per model.** Prices are USD per 1M tokens, Standard tier, ≤272K prompt (OpenAI) or base (others).
Benchmarks are the vendor's headline coding score, a tool-calling score where one exists, and one
long-horizon agentic score. No vendor publishes τ²-bench/BFCL/ToolBench for any of these models; the
closest published tool-use signals are AA's τ³-Banking and Toolathlon.

*OpenAI GPT-5.6 Luna (`gpt-5.6-luna`) — standard lane, light fallback.* Price $0.20 in / $0.02 cached /
$1.20 out (cut 80% on 2026-07-30; https://developers.openai.com/api/docs/pricing). Context 1,050,000 / 128K out.

| Effort | Coding | Tool calling | Long-horizon | Token efficiency | Source |
|---|---|---|---|---|---|
| max | SWE-Bench Pro 62.7% (3P); Terminal-Bench 2.1 84.7% (3P); DeepSWE 1.1 67.2% (3P) | unknown | OSWorld 2.0 45.6%, BrowseComp 83.3% (3P) | AA Intelligence Index 51 at $0.21/task; 130M output tokens for the whole AA index run ($213.83) | https://www.goml.io/blog/gpt-5-6-benchmarks ; https://artificialanalysis.ai/articles/gpt-5-6-has-landed ; https://artificialanalysis.ai/models/gpt-5-6-luna |
| xhigh (ours) | unknown per-effort | unknown | unknown | unknown | not found at https://openai.com/index/gpt-5-6/ or the model page |
| low / medium / high | unknown per-effort | unknown | unknown | unknown | same |

*OpenAI GPT-5.6 Sol (`gpt-5.6-sol`) — heavy lane today.* Price $4 / $0.40 / $20 (promotional "at least
through 2026-11-21"; launch was $5/$30; https://developers.openai.com/api/docs/models/gpt-5.6-sol). Context 1,050,000 / 128K.

| Effort | Coding | Tool calling | Long-horizon | Token efficiency | Source |
|---|---|---|---|---|---|
| max | AA Coding Agent Index 80 (V, "less than half the output tokens" of Fable 5); Terminal-Bench 2.1 88.8% (3P; 91.9% at `ultra`); SWE-Bench Pro 64.6% (3P); Terminal-Bench 4.0 37.3% (V) | unknown | OSWorld 2.0 62.6%, BrowseComp 92.2% (V) | AA II 59 at $1.04/task, ~15K output tokens per task (3P) | https://openai.com/index/gpt-5-6/ ; https://openai.com/index/gpt-6-astra/ ; https://artificialanalysis.ai/articles/gpt-5-6-has-landed |
| high (ours) | unknown per-effort | unknown | unknown | unknown | — |
| medium | AA II 46 (3P) | unknown | unknown | blended $3.08/M, 72 tok/s | https://artificialanalysis.ai/models/comparisons/gpt-6-astra-medium-vs-gpt-5-6-sol-medium |

*OpenAI GPT-5.6 Terra (`gpt-5.6-terra`) — suspended.* Price $2 / $0.20 / $12. SWE-Bench Pro 63.4%,
Terminal-Bench 2.1 87.4%, OSWorld 50.2% (3P, https://www.goml.io/blog/gpt-5-6-benchmarks). AA II 55 at
$0.55/task; AA's verdict: "for any Terra effort level, there is a Luna or Sol effort level that is more
intelligent at no extra cost" (https://artificialanalysis.ai/articles/gpt-5-6-intelligence-vs-cost-across-sol-terra-luna).
No per-effort numbers published.

*OpenAI GPT-6 Astra (`gpt-6-astra`) — directed heavy primary.* Price $10 / $1 / $50 (Fast mode 2×;
https://developers.openai.com/api/docs/pricing). Context 1,050,000 / 128K.

| Effort | Coding | Tool calling | Long-horizon | Token efficiency | Source |
|---|---|---|---|---|---|
| unstated (vendor table) | Terminal-Bench 4.0 57.9% (Sol 37.3%, Fable 5.1 55.8%); DeepSWE 1.1 74.1%; FrontierCode 1.1 64.5%; AA CAI 67.0 (Opus 5 68.1, Fable 5 67.2) | unknown (SOTA claimed on Agents' Last Exam, no number retrieved) | Terminal-Bench Science 64.6%; ExploitBench 100% | "~9% and 63% lower estimated API cost per task" than Sol and Fable 5.1 on TB 4.0 | https://openai.com/index/gpt-6-astra/ |
| max | AA II 61 (= Sol max), ~10% fewer output tokens than Sol, but 75% more expensive per task; uses ~1/3 the tokens of Sol max and ~1/5 of Opus 5 xhigh on the AA CAI | unknown | Epoch ECI 169, rank 1 | "various effort levels occupy the Pareto frontier of token efficiency" | https://artificialanalysis.ai/articles/benchmarking-gpt-6-astra ; https://epoch.ai/models/gpt-6-astra |
| high (ours) | unknown per-effort | unknown | unknown | unknown | — |
| medium | AA II 52; 61 tok/s; TTFT 9.65 s | unknown | unknown | unknown | https://artificialanalysis.ai/models/comparisons/gpt-6-astra-medium-vs-gpt-5-6-sol-medium |

*Anthropic Claude Fable 5.1 (`claude-fable-5-1`) — taste and supervisor.* Price $10 in / $50 out; cache
read $0.25 (0.025× — the one model with a deep cache-read discount); cache write $12.50 (5 min) / $20 (1 h)
(https://docs.anthropic.com/en/docs/about-claude/pricing). Context 1M / 128K out. Adaptive thinking; effort is
the only depth control. Default effort in Claude Code is **high**.

| Effort | Coding | Tool calling | Long-horizon | Token efficiency | Source |
|---|---|---|---|---|---|
| unstated (vendor table) | Terminal-Bench 4.0 55.8%; CursorBench 73.4% | unknown (AA: "+9 points over Fable 5 on τ³-Banking", no absolute) | OSWorld 2.0 77.9% partial / 41.7% strict; Terminal-Bench-Science 52.6% | "25% less than Fable 5 for typical workloads, up to ~45% for agentic work" (cache-read cut) | https://www.anthropic.com/claude-fable-and-mythos-5-1 ; https://artificialanalysis.ai/articles/claude-fable-5-1 |
| max | AA II 66 at $3.76/task; 143.7M output tokens on the index; Terminal-Bench 2.1 91.4% | unknown | HLE 59.1% | ~1.7× the output tokens of Fable 5 max | https://artificialanalysis.ai/articles/claude-fable-5-1 |
| xhigh | AA II 65 at $2.72/task | unknown | unknown | — | same |
| high (what actually runs) | unknown per-effort | unknown | unknown | unknown | — |
| medium (registry) | vendor prose only: "at Low or Medium effort, Fable 5.1 achieves results similar to or better than Fable 5's at a much lower cost" | unknown | unknown | — | https://www.anthropic.com/claude-fable-and-mythos-5-1 |
| low | AA II 58; 13.1M output tokens on the index (the five levels span **11×** in output tokens) | unknown | unknown | — | https://artificialanalysis.ai/articles/claude-fable-5-1 |

*Anthropic Claude Opus 5 (`claude-opus-5`) — fallback for taste/supervisor/standard.* Price $5 / $25;
cache read $0.50; cache write $6.25 / $10 (1 h). Context 1M / 128K. Thinking can be disabled only at effort ≤ high.

| Effort | Coding | Tool calling | Long-horizon | Token efficiency | Source |
|---|---|---|---|---|---|
| unstated (vendor) | Terminal-Bench 4.0 52.3%; CursorBench 70.0%; SWE-bench Verified 96.0 and SWE-bench Pro 79.2 (3P reading of the system card) | unknown | OSWorld 2.0 75.4% / 39.6% | customer quotes: "26% fewer tokens than Opus 4.8 at max" | https://www.anthropic.com/claude-fable-and-mythos-5-1 ; https://www.anthropic.com/news/claude-opus-5 ; https://jessemoraga.com/2026/07/25/claude-opus-5-benchmarks/ |
| max | AA II 61 at $2.03/task at launch (54 at $4.21 on index v4.2); Terminal-Bench 2.1 89% | unknown | HLE 53% | output tokens span ~8× low→max | https://artificialanalysis.ai/articles/opus-5 ; https://artificialanalysis.ai/models/claude-opus-5 |
| xhigh | AA CAI joint first place with Claude Code | unknown | unknown | — | https://artificialanalysis.ai/articles/opus-5 |
| high (ours) | unknown per-effort | unknown | unknown | unknown | — |
| low | "even at its lowest effort setting, Opus 5 passes more tasks than any other model" on AutomationBench (V prose) | unknown | unknown | — | https://www.anthropic.com/news/claude-opus-5 |

*Anthropic Claude Haiku 4.5 (`claude-haiku-4-5-20251001`) — light lane.* Price $1 / $5; cache read $0.10;
write $1.25 / $2. Context 200K / 64K. **No effort parameter**; the registry's `low` has no vendor meaning
for this model. SWE-bench Verified 73.3% with a 128K thinking budget (V); Terminal-Bench (Terminus 2)
40.21% without thinking, 41.75% with 32K (V); τ²-bench reported but the number is in an image (unknown);
AA II 22 at $0.20/task (https://www.anthropic.com/news/claude-haiku-4-5 ; https://artificialanalysis.ai/models/claude-4-5-haiku-reasoning).

*Anthropic Claude Sonnet 5 (reference, not routed).* $2 / $10, cache read $0.20; SWE-bench Verified
85.2, Terminal-Bench 2.1 80.4 (3P); effort low…max, medium "comparable to Sonnet 4.6 at high"
(https://docs.anthropic.com/en/docs/about-claude/pricing ; https://docs.anthropic.com/en/docs/build-with-claude/effort).

*xAI Grok 4.5 (`grok-4.5`) — Grok default.* Price $2 / $0.30 cached / $6 (≥200K prompt: $4 / $0.60 / $12);
context 500K (https://docs.x.ai/developers/models/grok-4.5). Only `high` is benchmarked: DeepSWE 1.0 62.0%
(AA-run, in the vendor post); SWE-Bench Pro, Terminal-Bench 2.1 and τ³-Banking exist in the model card PDF
but did not extract (unknown). Token efficiency is the vendor's headline: 15,954 output tokens per
SWE-Bench Pro task vs Opus 4.8's 67,020 (https://x.ai/news/grok-4-5); AA: 64M output tokens for the index
(median model 79M), $0.31/task, AA CAI 76 at $2.49/task and 1.9M tokens/task vs Fable 5 in Claude Code
7.2M/$11.80 (https://artificialanalysis.ai/models/grok-4-5). OpenCode exposed no low/medium/high variants for
grok-4.5 at retrieval (https://github.com/anomalyco/opencode/issues/39448).

*Alibaba Qwen 3.8 Max (`qwen3.8-max`, OpenCode) — receipt-gated.* Price $2 / $6, implicit cache $0.25
(international; $1.65/$4.95 in some regions; https://github.com/AlibabaCloud-Official/Qwen3.8-max ;
https://www.alibabacloud.com/help/en/model-studio/qwen3-8-max). Context 1M / 131K out. Thinking-on only:
SWE-bench Pro 67.7 (Claude Code harness), Terminal-Bench 2.1 86.6, Toolathlon Verified 72.5 (the one
vendor tool-calling number in this set), OSWorld-Verified 86.1 (V via mirrors:
https://go.tabbit.ai/model/qwen3-8-max/reviews/qwen-official-release-notes-and-complete-performance-results);
AA τ³-Banking 51.3% (2nd), AA II 47 but 150M output tokens on the index — "very verbose"
(https://artificialanalysis.ai/models/qwen3-8-max). OpenCode's built-in `alibaba` provider dropped
`enable_thinking`/`thinking_budget` from the wire in 1.18.25 (https://github.com/anomalyco/opencode/issues/46647),
so OpenCode "effort" for this model was a no-op at retrieval.

**What the published data cannot tell us.** (1) Nobody publishes per-effort scores for `high` or `xhigh`
on the models we run at those levels; the only per-effort ladders are AA's (Fable 5.1 low→max: 58→66 for
11× tokens; Opus 5 ~8× tokens; Sol/Astra medium vs max). (2) Tool-calling benchmarks are absent from every
vendor page except Qwen's Toolathlon and AA's τ³-Banking. (3) Astra's per-effort behaviour is a vendor
chart without numbers.

### B. What this host measured

Method and definitions are in `2026-09-06-model-lane-history.md`; rows in the `.csv`. Sources: Codex
rollouts in all three Codex homes (`~/.codex`, `~/.codex-support@gabber.studio`, `~/.codex-pippenz@gmail.com`;
per-turn token counts and every tool call), Claude transcripts in all three Claude homes (per-message
usage), `cas.db` task notes (`request_changes` decisions), `spawn_queue.worker_spec`, and factory-session
logs (urgent stops). Cost is a shadow price at 2026-09-06 list; both harnesses actually run on subscriptions.

| Lane as run (model / effort) | Deliveries | Workers | Send-backs | Send-back rate | Urgent stops | Uncached in / delivery | Cached in / delivery | Output / delivery | Tool calls / delivery | Median min to first push | Cost / merged delivery @ list |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| standard: Luna / xhigh | 212 | 133 | 26 | 12% | 25 | 401,318 | 17,757,010 | 46,395 | 139 | 17.0 | **$0.49** |
| heavy: Sol / high | 51 | 23 | 8 | 16% | 20 | 254,657 | 12,649,266 | 36,353 | 103 | 12.1 | **$6.81** |
| heavy as directed: Astra / high | 2 | 1 | 0 | 0% | 0 | 172,321 | 9,160,512 | 33,220 | 74 | 19.1 | **$12.54** |
| Astra / medium | 1 | 1 | 1 | 100% | 1 | 147,945 | 9,859,456 | 26,733 | 95 | 11.8 | **$12.68** |
| former standard: Terra / high (08-20 only) | 42 | 34 | 1 | 2% | 0 | 199,407 | 8,134,254 | 21,041 | 69 | 6.2 | **$2.28** |
| former taste / fallback: Opus 5 / high | 52 | 26 | 2 | 4% | 13 | 283 | 40,020,008 | 93,633 | 73 | 16.4 | **$25.47** |
| subset 09-05 16:20Z→ (Fable-supervised): Luna / xhigh | 24 | 13 | 3 | 12% | 2 | 360,590 | 15,875,541 | 39,639 | 122 | 18.9 | $0.44 |
| subset 09-05 16:20Z→: Sol / high | 1 | 1 | 1 | 100% | 0 | 472,340 | 33,266,432 | 59,648 | 224 | 51.3 | $16.39 |
| subset 09-04 22:22Z→09-05 16:20Z (Astra-supervised): Sol / high | 36 | 11 | 7 | 19% | 18 | 226,459 | 12,003,428 | 37,631 | 101 | 13.9 | $6.46 |
| light: Haiku / low; taste: Fable as worker | 0 | 0 | — | — | — | — | — | — | — | — | — |

Two findings the first pass missed because it read only one Codex home and one Claude home:

**The 2026-09-05 stall was Codex GPT-6 Astra at medium.** The supervisor that held finished workers from
~05:56Z to 15:10Z (agent loyal-crane-48, cas-20a3 note 15:10Z) is rollout
`~/.codex-support@gabber.studio/sessions/2026/09/04/rollout-2026-09-04T18-22-16-01a06e83…`: Sol/high for
17 minutes, Astra/high from 22:39Z, Astra/medium from 00:54Z (the `~/.codex/config.toml` default). The
rollout has no events between 05:56Z and 11:48Z (352 minutes), and its continuation session (13:16–16:16Z,
Astra/high then medium) has 34- and 65-minute gaps before the operator stepped in. The night cost $315.31
at Astra list (86% of it cached input at $1/M; 1,345 tool calls; 197K output) — 2.2× the live Fable
supervisor day. The deliveries it reviewed carry the horizon's worst outcomes: Sol 7 send-backs and 18
urgent stops on 36 deliveries (19%), Luna 3 of 4 sent back, 11 urgent stops on one Opus worker. Whether
that is the reviewer or the tasks, the rows cannot say; Sol's whole-horizon 16% send-back rate is
mostly that night (P2 Sol: 0 send-backs on 10).

**Every supervisor session ran Fable at effort=high, never medium.** All 17 Claude supervisor sessions
since 2026-08-20 (three homes, Fable 5 then Fable 5.1) carry `effort: high` on every record; the
registry's `supervisor` and `taste` lanes say `medium` and 3.17.2 shipped that default. Claude Code
defaults to high and the spawn path never sets `effortLevel`, so the lane default in 3.17.2 differs
from what produced every result in this brief. The transcripts cannot price the difference — no medium
session exists; AA's ladder puts Fable 5.1 low→max at 11× output tokens. The live supervisor day costs
$142 at list (50% 1-hour cache writes at $20/M, 39% cache reads, 11% output) against $10.5 for the 24
Luna deliveries it reviewed.

Astra as a worker: three deliveries on the Astra night (fair-swan-9 high ×2, 0 send-backs; fierce-dragon-53
medium ×1, 1 send-back) at $12.5 each — 74–95 tool calls and 27–36K output tokens per delivery, in Sol's
range, at 1.9× Sol's list cost per delivery. A sample, not a rate.

### B.2 Every project, 2026-08-20 → 2026-09-06 (cas-e208 extractor, all six harness homes)

Source: `scripts/factory-model-history.py` (task cas-e208) run three times — once per Codex/Claude home
pair, because it accepts one root of each — and unioned by `scripts/factory-model-history-union.py` into
`docs/factory/data/factory-model-history-2026-09-06-allhomes-horizon.csv` (1,498 session×task rows in the
horizon) and `…-scorecard-2026-09-06-allhomes-horizon.csv`. Prices come from
`docs/factory/data/model-prices.json`, filled from PART A; the extractor's `apply_costs` was corrected to
bill Codex cached tokens once (Codex reports `cached_input_tokens` as a subset of `input_tokens`).

**Extractor definitions differ from B above, and the columns say so.** Unit = one worker × one task from the
project database (`spawn_queue` + leases), joined to a transcript by worker name; a session working two
tasks appears twice with the same tokens, so cost per delivered task divides each session's cost once.
Send-backs = every `request_changes` / `changes requested` mention in the task's notes (worker progress
notes quoting the decision count too, so rates can exceed 100%). Tool calls = Codex `function_call`
(MCP) only — shell `custom_tool_call`s are not counted, which is why the medians are ~4× lower than in B.
Minutes to first push = factory-log "pushed" marker, present for a minority of rows. Urgent stops = note
text, not log events (hence 0). **Miss rate** = sessions with no transcript tokens; the 837 DB-only rows
(56% of the horizon) carry no model at all because the spawn row has no `worker_spec` — they are shown,
not dropped.

| Model / effort (all projects) | Sessions | With tokens | Miss rate | Tasks delivered | Send-back mentions | Rate | Median min to first push | Median output / delivered task | Median MCP calls | Cost / delivered task @ list |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| gpt-5.6-luna / xhigh | 349 | 332 | 5% | 272 | 78 | 29% | 25.6 | 54,109 | 32 | **$0.69** |
| gpt-5.6-terra / high | 68 | 66 | 3% | 44 | 14 | 32% | 11.9 | 23,520 | 22 | **$3.53** |
| gpt-5.6-sol / high | 66 | 63 | 5% | 57 | 38 | 67% | 18.3 | 33,102 | 28 | **$7.10** |
| gpt-6-astra / high | 1 | 1 | 0% | 1 | 0 | 0% | — | 30,577 | 28 | $12.27 (see B for the 3-delivery sample) |
| claude-haiku-4-5 / low | 22 | 15 | 32% | 22 | 0 | 0% | 19.6 | 36,302 | 32 | **$1.46** |
| claude-opus-5 / high | 118 | 86 | 27% | 111 | 25 | 23% | 66.2 | 178,640 | 171 | **$43.93** |
| claude-fable-5-1 / high (worker) | 6 | 5 | 17% | 2 | 0 | 0% | 20.6 | 214,691 | 74 | $130.00 |
| claude-fable-5-1 / medium (worker) | 4 | 4 | 0% | 3 | 4 | 133% | 57.7 | 691,080 | 288 | $103.02 |
| claude-fable-5 / high | 14 | 14 | 0% | 7 | 11 | 157% | 35.4 | 5,456 | 9 | unpriced |
| (no model — DB row without worker_spec) | 837 | 0 | 100% | 703 | 120 | 17% | — | — | — | — |

| Project | Model / effort | Sessions | Miss | Delivered | Send-back mentions | Rate | Median min to push | Median output | Median MCP calls | Cost / delivered |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| cas-src | gpt-5.6-luna / xhigh | 159 | 1% | 124 | 39 | 31% | 25.9 | 62,116 | 35 | $0.83 |
| cas-src | gpt-5.6-sol / high | 31 | 0% | 28 | 16 | 57% | 27.3 | 31,926 | 28 | $7.99 |
| cas-src | gpt-5.6-terra / high | 34 | 0% | 22 | 1 | 5% | 11.9 | 23,788 | 20 | $4.42 |
| cas-src | claude-opus-5 / high | 35 | 0% | 34 | 7 | 21% | 42.2 | 213,544 | 185 | $55.77 |
| cas-src | claude-fable-5-1 / high | 5 | 0% | 1 | 0 | 0% | 24.0 | 214,691 | 74 | $130.00 |
| cas-src | claude-fable-5-1 / medium | 4 | 0% | 3 | 4 | 133% | 57.7 | 691,080 | 288 | $103.02 |
| cas-src | (no model) | 385 | 100% | 322 | 58 | 18% | — | — | — | — |
| gabber-studio | gpt-5.6-luna / xhigh | 52 | 0% | 45 | 13 | 29% | 18.2 | 36,281 | 28 | $0.30 |
| gabber-studio | gpt-5.6-sol / high | 15 | 0% | 15 | 14 | 93% | — | 71,454 | 48 | $3.85 |
| gabber-studio | claude-opus-5 / high | 57 | 42% | 51 | 10 | 20% | 31.8 | 120,261 | 122 | $35.49 |
| gabber-studio | claude-haiku-4-5 / low | 16 | 6% | 16 | 0 | 0% | 19.6 | 36,302 | 32 | $1.46 |
| gabber-studio | (no model) | 187 | 100% | 166 | 21 | 13% | — | — | — | — |
| ozer | gpt-5.6-terra / high | 8 | 0% | 4 | 2 | 50% | 10.9 | 23,945 | 20 | $2.85 |
| abundant-mines | gpt-5.6-luna / xhigh | 26 | 0% | 22 | 2 | 9% | 23.5 | 70,842 | 42 | $0.93 |
| abundant-mines | claude-opus-5 / high | 4 | 0% | 4 | 1 | 25% | — | 46,068 | 50 | $7.36 |
| abundant-mines | claude-haiku-4-5 / low | 4 | 100% | 4 | 0 | 0% | — | — | — | — |
| abundant-mines | (no model) | 51 | 100% | 44 | 2 | 5% | — | — | — | — |
| rocketship-template | any | 0 in horizon (its 155 spawns predate 2026-08-20) | — | — | — | — | — | — | — | — |
| Penguinz | gpt-5.6-luna / xhigh | 19 | 11% | 9 | 0 | 0% | 47.7 | 52,063 | 35 | $0.75 |
| Penguinz | gpt-5.6-terra / high | 20 | 10% | 13 | 3 | 23% | 9.1 | 17,946 | 22 | $2.50 |
| Penguinz | (no model) | 43 | 100% | 26 | 4 | 15% | — | — | — | — |
| Woodworking | claude-opus-5 / high | 10 | 60% | 10 | 6 | 60% | 216.9 | 421,602 | 174 | $39.22 |
| Woodworking | gpt-5.6-luna / xhigh | 5 | 0% | 3 | 7 | 233% | 27.9 | 46,358 | 18 | $0.68 |
| Woodworking | gpt-5.6-sol / high | 4 | 0% | 2 | 8 | 400% | 7.1 | 14,392 | 10 | $3.36 |
| Woodworking | gpt-5.6-terra / high | 4 | 0% | 4 | 8 | 200% | 20.9 | 27,960 | 26 | $2.86 |
| Woodworking | (no model) | 21 | 100% | 13 | 17 | 131% | — | — | — | — |
| pulse-card | gpt-5.6-luna / xhigh | 31 | 6% | 25 | 6 | 24% | 24.2 | 48,249 | 30 | $0.60 |
| pulse-card | claude-opus-5 / high | 5 | 0% | 5 | 0 | 0% | — | 164,259 | 212 | $20.07 |
| pulse-card | (no model) | 51 | 100% | 46 | 5 | 11% | — | — | — | — |
| petra-stella-cloud | gpt-5.6-luna / xhigh | 36 | 3% | 30 | 10 | 33% | 30.5 | 57,731 | 37 | $0.63 |
| petra-stella-cloud | gpt-5.6-sol / high | 10 | 20% | 7 | 0 | 0% | 17.8 | 25,480 | 26 | $6.13 |
| petra-stella-cloud | claude-opus-5 / high | 5 | 0% | 5 | 1 | 20% | 91.6 | 417,503 | 234 | $65.91 |
| mecha_cassy | gpt-5.6-luna / xhigh | 19 | 53% | 14 | 1 | 7% | 26.4 | 71,381 | 48 | $0.54 |

Rows with fewer than three sessions are omitted; the full set is in the scorecard CSV. Horizon totals at
list (unique sessions): Opus 5 $3,470 for 111 delivered tasks; Sol $398 for 57; Luna $184 for 272; Terra
$156 for 44; Fable 5.1 workers $439 for 5; Haiku $22 for 22.

**Reconciliation with B (cas-src).** Same sessions, different units, and the differences are all
definitional:

| Metric, cas-src | B (this task's extraction) | B.2 (cas-e208 extractor) | Why |
|---|---:|---:|---|
| Luna/xhigh deliveries | 212 task segments | 124 delivered tasks (159 sessions) | B splits a session at each `task start` and counts continuations; B.2 counts distinct closed task ids joined by worker name, and 11 Luna sessions never matched a transcript |
| Luna send-backs | 26 (12%) | 39 mentions (31%) | B counts the `Decision: changes requested` line once per task; B.2 counts every mention, including worker notes that quote it |
| Sol/high send-backs | 8 of 51 (16%) | 16 of 28 (57%) | same; Sol's rows carry more quoted decisions per task |
| Luna tool calls / delivery | 139 | 35 | B counts shell + MCP calls; B.2 counts `function_call` (MCP) only |
| Luna minutes to first push | 17.0 median | 25.9 median | B: first `git push` tool call after task start; B.2: factory-log "pushed" marker per worker |
| Luna cost / delivery | $0.49 | $0.83 | same token totals, divided by segments (212) vs delivered tasks (124); Luna cas-src total $104 (B) vs $128 (B.2, 159 sessions incl. 2026-08-25 sessions B filtered as `(none)`) |
| Opus 5 cost / delivery | $25.47 | $55.77 | 52 segments vs 34 delivered tasks; totals $1,325 vs $1,932 (B.2 includes 09-02/09-04 sessions B attributed to no task) |
| Terra cost / delivery | $2.28 | $4.42 | totals agree ($95.68 vs $97.16); 42 segments vs 22 delivered tasks |
| Astra deliveries | 3 (fair-swan-9 ×2, fierce-dragon-53) | 1 (fair-swan-9, cas-1939) | B.2 joins one task per spawn row; fair-swan-9's second task and fierce-dragon-53's transcript did not join |
| Astra/medium supervisor stall | named (B) | absent | the extractor joins worktree cwds only; main-checkout supervisor sessions are outside its unit |

Totals agree to within the sessions each side attributes; per-delivery figures differ by the denominator.
Use B for per-delivery token/tool economics and stall evidence, B.2 for cross-project breadth.

### C. Where each model shines (re-cut for the operator's Option A: heavy = Astra/high, placed by hand)

One row per model × effort with the five measures the operator asked for, each with its source and
sample size. No single ranking: the columns disagree, and that is the finding.

| Model / effort | Send-back rate | Cost / delivery @ list | Minutes to first push | Stall / urgent-stop incidents | Tool calls / delivery | Where it shines | Where it does not |
|---|---:|---:|---:|---|---:|---|---|
| Luna / xhigh | 12% (B, n=212); 29% mentions (B.2, n=272) | $0.49 (B) / $0.69 (B.2) | 17.0 (B) / 25.6 (B.2) | 25 urgent stops on 133 cas-src workers; 0 stalls | 139 shell+MCP (B) / 32 MCP (B.2) | **volume implementation in every project**: cheapest by 5–60×, 272 deliveries across 9 projects, 0–9% send-backs in Penguinz, abundant-mines, mecha_cassy | cas-src and petra-stella-cloud, where it carries the most send-backs in absolute terms (39, 10); slowest first push of the Codex models |
| Sol / high | 16% (B); 67% mentions (B.2) — 0% in cas-src P2 and petra-stella-cloud, 19% on the Astra night, 93% in gabber-studio | $6.81 (B) / $7.10 (B.2) | 12.1 (B) / 18.3 (B.2) | 20 urgent stops (18 on the Astra night) | 103 (B) / 28 MCP | **fast first push and clean deliveries under a Claude supervisor** (0 of 10 in P2, 0 of 7 in petra-stella-cloud) | reviewed by Astra or on gabber-studio it collects more send-back mentions than any model; 14× Luna's cost |
| Terra / high (suspended) | 2% (B, n=42); 32% mentions (B.2, n=44) | $2.28 / $3.53 | 6.2 (B) / 11.9 (B.2) | 0 | 69 / 22 MCP | fastest first push on the host; cleanest cas-src record (1 send-back in 42); AA: dominated by a Luna or Sol effort on intelligence per dollar | suspended 2026-08-27 by operator decision; Woodworking and ozer rows are 200% / 50% mentions |
| **Astra / high** (Option A heavy) | 0% (B, n=2) | $12.54 (B) / $12.27 (B.2) | 19.1 (B) | 0 as worker | 74 (B) / 28 MCP | the only routed model with a **published** intelligence gain (TB 4.0 57.9 vs Sol 37.3, AA CAI 67.0) and AA's "Pareto frontier of token efficiency"; two clean cas-src deliveries at 27–36K output tokens | n=2; 1.8× Sol and 25× Luna per delivery; its cached-input price ($1/M) makes long-context roles 4× a Fable seat |
| **Astra / medium** | 100% (n=1 worker); as supervisor: reviewed lanes at 19–75% | $12.68 (worker) / $315 per supervisor night | 11.8 | **352-minute stall** as supervisor (09-05); 1 urgent stop as worker | 95 / 1,345 per night | nothing measured | **never as a driver**: the only medium session on record is the stall; as the Codex default effort it is what an unpinned `codex` spawn gets |
| Opus 5 / high | 4% (B, n=52); 23% mentions (B.2, n=111) | $25.47 (B) / $43.93 (B.2) | 16.4 (B) / 66.2 (B.2) | 13 urgent stops (11 on one worker) | 73 (B) / 171 MCP | **judgment and design work in gabber-studio and pulse-card** (20% / 0% mentions, 122–212 MCP calls per task — it talks to Cassy, not the shell); abundant-mines 25% at $7.36 | Woodworking (217 min to push, $39, 60%) and petra-stella-cloud ($65.91); 52–64× Luna per delivery; highest output tokens of any worker model (179K median) |
| Fable 5.1 / high (worker) | 0% (n=5 taste tasks: skill rewrites, design language) | $22–$52 per delivery (B.2 rows) | 20.6 | 0 | 67–93 MCP | **taste tasks that end in one delivery**: five cas-src skill/design rewrites, none sent back | $130 per *delivered* task because 3 of 5 are still open; 1-hour cache writes dominate |
| Fable 5.1 / medium (worker) | 2 tasks with 3 mentions (tender-panda-58: hub visual overhaul, Slack bridge fix) | $103 per delivered (one 993K-output, 462-call session) | 57.7 | 0 | 462 | the one measured medium session delivered a full UI overhaul | 3.3× the tokens of the median Fable/high worker; cannot be separated from the task's size |
| Fable 5.1 / high (supervisor) | reviewed Luna at 12% | $142–$284 per session-day | — | 0 stalls in 17 sessions | 70–150 per day | **coordination without stalls**, cache reads at $0.25/M | the most expensive seat after the Astra night; the registry's `medium` has never run |
| Haiku 4.5 / low | 0% (B.2, n=22; 15 with tokens) | **$1.46** | 19.6 | 0 | 32 MCP | **the light lane exists after all**: 15 gabber-studio Slack release-note postings at $0.51–$4.26 each, none sent back | never used outside gabber-studio + abundant-mines; `low` sets nothing on Haiku |

**Where to place Astra/high (Option A).** The evidence supports placing it, by hand, on tasks where
the published intelligence gap matters and the run is short: safety-relevant implementation with a
bounded brief (its two deliveries were 27–36K output tokens, 69–78 tool calls, first push in 18–20
minutes). It does not support Astra on anything long-context or coordinating: cached input is 86% of
its cost and its only medium session is the stall. Concretely:

1. **Heavy = Astra/high with Sol/high as fallback, as directed; pin `effort=high` in the recipe and
   refuse medium for heavy.** Every unpinned `codex` spawn inherits `~/.codex/config.toml`'s
   `gpt-6-astra` / `medium`; the registry must not let heavy degrade to that.
2. **Measure the first five Astra/high heavy deliveries against Sol/high pairs** (same brief, both
   lanes) on send-backs, output tokens, tool calls and minutes to first push from the rollouts. Two
   deliveries is a sample; five paired is a decision.
3. **Luna/xhigh stays standard everywhere and is the rescue lane for heavy** — 272 deliveries across
   nine projects at $0.69, the lowest send-back rates outside cas-src, and it finished cas-c674 after
   Sol.
4. **Light = Haiku 4.5 for Slack/release-note and posting chores** — 22 deliveries, 0 send-backs,
   $1.46 — and give the recipe a `thinking_budget` instead of `low`. Route anything that touches code
   to Luna.
5. **Taste = Fable 5.1/high for skill, design and document rewrites** (5 of 5 clean); keep Opus 5/high
   as the taste fallback where it has a record (gabber-studio, pulse-card), not for Woodworking-style
   long jobs. Supervisor stays Fable 5.1 and the registry should say `high` until a medium day is
   measured.
6. **Fix attribution before the next scorecard**: 56% of horizon sessions have no model because the
   spawn row lacks `worker_spec`; the extractor should read the rollout's `turn_context` for those, and
   it should count shell tool calls and read all six harness homes in one run.

What the data cannot answer: Astra/high vs Sol/high on identical tasks (item 2); Fable medium vs high
on identical work (one medium worker session, zero medium supervisor sessions); whether Sol's 93% in
gabber-studio and the Astra night's 19% are the reviewer or the tasks; and whether Opus 5's 179K-token
deliveries are thoroughness or verbosity — the send-back rate says the former in gabber-studio and the
latter in Woodworking.

## Where the rubric fails

1. **Heavy is routed on reputation against the data.** Astra has zero worker deliveries in the
   window and one recorded coordination stall. Sol has one delivery and one send-back. Luna has
   nineteen merged deliveries and is not in heavy at all.
2. **Silent fallback to the replaced default.** Opus 5/high was the built-in supervisor default until
   2026-09-05 22:00Z. It is now the automatic backup for both judgment lanes. A bad auth hour changes
   the coordinator's behaviour with nobody deciding it. cas-255e adds a loud receipt; loud is not
   approved.
3. **Effort has no stated rationale.** Luna xhigh only; Fable medium; Astra medium by recipe but
   high in heavy; Sol high; Haiku low. A reader cannot tell cost decisions from quality decisions.
4. **Nothing is measured.** Every routing change this week (Astra→Fable taste, Fable supervisor,
   Opus fallbacks, Astra heavy) came from anecdote. The actionable-idle metric that shipped in 3.17.2
   is the first number the rubric has ever had.
5. **The light lane is decorative.** Zero uses in the window. Every mechanical chore went to Luna
   because Haiku is not trusted with builtin marker tests.
6. **Cross-harness fallback inside a lane.** Standard falls back from Codex to Claude: different
   hooks, skill mirror, and account mid-epic.
7. **Fallback edges are declared but disabled.** Until cas-255e lands, taste and supervisor carry a
   fallback that never fires; the comment says "fail closed". A registry that documents one policy
   and executes another is a review hazard.

## Optimizations, in order of leverage

1. **Measure three numbers per lane, per week**, from data CAS already records:
   send-backs per delivery (task notes carry request_changes), actionable-idle minutes (3.17.2
   metric), and assignment-to-first-push minutes (lease start vs first pushed tip). Print them in the
   generated route table so every reader sees the rubric and its scorecard together.
2. **Promote by trial, not by directive.** Keep Sol as heavy primary; make Astra/high the heavy
   fallback for two weeks; promote only if its send-back rate is at or below Sol's on at least five
   deliveries.
3. **Make fallbacks explicit decisions.** A lane fallback fires only after the supervisor is told
   the primary is unavailable and the receipt names both recipes. For the supervisor lane, prefer
   fail-closed plus an operator alert over a silent model change.
4. **Add an effort column with a reason.** One sentence per lane: why this effort, what it costs
   relative to the lane below, and what evidence would change it.
5. **Retire or re-scope light.** Either route it at Luna/xhigh for tiny chores (which is what
   happens today) or give Haiku a bounded class of work with its own marker tests.
6. **Keep fallbacks inside a harness.** Standard should fall back to another Codex recipe or fail
   closed; cross-harness fallbacks belong to the operator, not the registry.
7. **Give Luna a seat in heavy.** On this window's evidence it is the safest implementation lane we
   have; at minimum it should be the heavy fallback ahead of Sol.

## Decision requested

Keep cas-255e as directed (it is implementing the rubric above), and choose one of:

- A. Ship it as directed and start the three-number scorecard now; revisit in two weeks.
- B. Amend cas-255e so heavy stays on Sol with Astra/high as fallback, and add the scorecard.

The author recommends B.

## Provenance

- Registry: `crates/cas-factory/policy/lane-registry.toml` at epic 736bb1fe64176204b77e46b960ad23fba7d8cbba.
- Delivery and send-back counts: supervisor merge and review notes on epic cas-80b6, 2026-09-05 16:20Z–2026-09-06 00:10Z; task notes on cas-4626, c650, 62ca, bd04, bddf, a49c, 41ae, d05f, 47ea, e159, c674, 9eae1, 72f7, 16ee, a65d, 6e24, 1e85, 826a.
- Stall: cas-20a3 note 15:10Z; operator message 2026-09-05 17:27Z.
- Release timings: `/home/pippenz/.cas/artifacts/release/v3.17.1-epic-80b6-merge/FINAL-HANDOFF.md` and `.../v3.17.2-epic-80b6-merge/FINAL-HANDOFF.md`.
- Directive: operator message 2026-09-06 00:10Z; task cas-255e.
- Model section (cas-3372): external figures retrieved 2026-09-06 via exa-search from the URLs cited inline;
  internal figures from Codex rollouts in `~/.codex`, `~/.codex-support@gabber.studio` and `~/.codex-pippenz@gmail.com` (`sessions/2026/08/20…09/06`) (`token_count`, `turn_context`, tool
  calls), Claude transcripts in `~/.claude-daniel@petrastella.io`, `~/.claude-alt`, `~/.claude-pippenz@gmail.com` (`projects/…cas-src*`; per-message usage; all cache
  writes 1-hour TTL), `.cas/cas.db` task notes and `spawn_queue`, `.cas/logs/factory-session-*.log`;
  row-level data in `docs/factory/2026-09-06-model-lane-history.csv`, method in the `.md` beside it.
- Cross-project (cas-de0b): `scripts/factory-model-history.py` (cas-e208) run per home pair, unioned by `scripts/factory-model-history-union.py`; `docs/factory/data/model-prices.json`; `docs/factory/data/factory-model-history-2026-09-06.csv` (default homes, 9,259 rows), `…-allhomes-horizon.csv` (1,498 rows) and `…-scorecard-2026-09-06-allhomes-horizon.csv`; extractor receipt `~/.cas/artifacts/cas-e208/real-run-v2.log`.
- Stalled supervisor identity: `~/.codex-support@gabber.studio/sessions/2026/09/04/rollout-2026-09-04T18-22-16-01a06e83-f206-7f60-8a41-1c70f3fd1132.jsonl` (`turn_context` model/effort; timestamp gap 05:56Z→11:48Z) and its continuation `…/2026/09/05/rollout-2026-09-05T09-16-26-01a071b6-94d5-7061-b3b5-d6f136504a51.jsonl`; factory log `cas-src-daring-badger-54`, agent loyal-crane-48.
