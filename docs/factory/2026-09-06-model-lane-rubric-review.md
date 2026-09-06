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
`2026-09-06-model-lane-history.md` + `.csv`), and the synthesis (C). Every external number carries its
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

Method and definitions are in `2026-09-06-model-lane-history.md`; rows in the `.csv`. Source: Codex
rollouts (`~/.codex/sessions`, per-turn token counts and every tool call), Claude transcripts
(`.claude-daniel@petrastella.io/projects/…cas-src*`, per-message usage), `cas.db` task notes
(`request_changes` decisions), `spawn_queue.worker_spec`, and factory-session logs (urgent stops).
Cost is a shadow price at 2026-09-06 list; both harnesses actually run on subscriptions.

| Lane as run (model / effort) | Deliveries | Workers | Send-backs | Send-back rate | Urgent stops | Uncached in / delivery | Cached in / delivery | Output / delivery | Tool calls / delivery | Median min to first push | Cost / merged delivery @ list |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| standard: Luna / xhigh | 173 | 111 | 16 | 9% | 4 | 416,114 | 18,359,369 | 47,512 | 143 | 18.5 | **$0.51** |
| heavy: Sol / high | 12 | 10 | 1 | 8% | 0 | 366,598 | 16,434,624 | 38,118 | 125 | 6.9 | **$8.80** |
| former standard: Terra / high (08-20 only) | 42 | 34 | 1 | 2% | 0 | 199,407 | 8,134,254 | 21,041 | 69 | 6.2 | **$2.28** |
| former taste: Opus 5 / high | 38 | 21 | 0 | 0% | 2 | 303 | 41,779,207 | 102,080 | 78 | 16.2 | **$26.52** |
| subset 09-05 16:20Z→: Luna / xhigh | 20 | 9 | 3 | 15% | 2 | 403,603 | 18,420,544 | 44,525 | 138 | 18.9 | $0.50 |
| subset 09-05 16:20Z→: Sol / high | 1 | 1 | 1 | 100% | 0 | 472,340 | 33,266,432 | 59,648 | 224 | 51.3 | $16.39 |
| heavy as directed: Astra / high | 0 | 0 | — | — | — | — | — | — | — | — | est. **$22** at Sol's token profile |
| light: Haiku / low; taste: Fable as worker | 0 | 0 | — | — | — | — | — | — | — | — | — |

Supervisor sessions (Claude, main checkout): all eight sessions since 2026-08-27 ran at **effort=high**,
including every Fable 5.1 session; the registry's `medium` has never executed. The 2026-09-05 supervisor
day cost $131.78 at list (cache writes 53% at the 1-hour TTL rate, cache reads 36%, output 11%) against
$10.05 for the 20 Luna deliveries it reviewed. The 2026-09-03 session (the Opus burst's supervisor) cost
$284.15 with 976K output tokens.

Astra has no cas-src rollout in the retained horizon; the "9 h stall" attributed to it cannot be
re-measured from transcripts on this host. The Opus burst's 0 send-backs on 38 deliveries sits beside
2 urgent stops and a supervisor that may have rejected by message; treat it as unmeasured, not clean.

### C. Cost × intelligence × efficiency per lane at its configured effort

| Lane | Route | Published intelligence (coding) | Published tool calling | Measured send-back rate | Measured output tokens / delivery | Measured tool calls / delivery | Measured cost / delivery @ list | Verdict |
|---|---|---|---|---:|---:|---:|---:|---|
| standard | Luna / xhigh | SWE-Bench Pro 62.7%, TB 2.1 84.7% (3P, max) | unknown | 9% (n=173) | 47,512 | 143 | $0.51 | cheapest delivery on the host by 4× (Terra) to 50× (Opus); most send-backs in absolute terms, lowest cost per send-back |
| heavy (today) | Sol / high | AA CAI 80, TB 2.1 88.8% (max) | unknown | 8% (n=12) | 38,118 | 125 | $8.80 | 17× Luna's cost for a send-back rate inside Luna's noise; fastest first push (6.9 min) |
| heavy (directed) | Astra / high | TB 4.0 57.9% vs Sol 37.3%; AA: same II as Sol max at 75% more per task | unknown | no data | no data | no data | ~$22 (est.) | the only lane whose intelligence gain is published; zero local evidence; 2.5× Sol at list |
| heavy (alternative) | Luna / xhigh | as standard | unknown | 9% | 47,512 | 143 | $0.51 | already the de-facto rescue lane (finished cas-c674 after Sol) |
| taste + supervisor (registry) | Fable 5.1 / medium | vendor: "similar to or better than Fable 5 at much lower cost"; AA: low 58 → max 66 | unknown | never ran | never ran | never ran | — | untested; every real session was high |
| taste + supervisor (actual) | Fable 5.1 / high | unknown per-effort; xhigh AA II 65 | unknown | — | 290K–976K per session | 70–150 per session | $132–$284 per session | the most expensive seat in the factory; 1-h cache writes dominate |
| fallback | Opus 5 / high | TB 4.0 52.3%; SWE-bench Verified 96.0 (3P) | unknown | 0% (n=38, unverified) | 102,080 | 78 | $26.52 | 2× Luna's output tokens per delivery, half the tool calls, 52× the cost |
| light | Haiku 4.5 / low | SWE-bench Verified 73.3% (128K thinking) | τ²-bench unknown | never ran | — | — | est. ≤$1 | `low` is meaningless on Haiku (no effort dial); the lane needs a thinking budget, not an effort |

**Recommendations, in order of evidence.**

1. **Heavy: keep Sol/high primary, make Luna/xhigh the first fallback, Astra/high explicit-only until it
   has five measured deliveries.** Data: Sol 8% send-backs on 12 at $8.80; Luna 9% on 173 at $0.51; Astra
   0 deliveries and a list price 2.5× Sol. Astra's published edge (TB 4.0 57.9 vs 37.3, AA "Pareto frontier
   of token efficiency") is real and is exactly the reason to *measure* it: run the next five heavy tasks
   as Astra/high and Sol/high pairs on the same brief and compare send-backs, tool calls and output tokens
   from the rollouts. Luna/xhigh in heavy is not supported by any published intelligence number, only by
   the host's send-back data, so it belongs as fallback, not primary.
2. **Supervisor: measure Fable medium before deciding — and fix the registry to say what runs.** Every
   supervisor session ran high because Claude Code defaults to high and the spawn path never sets
   `effortLevel`. Vendor prose says medium ≈ Fable 5; AA says low→max spans 11× output tokens. A
   supervisor day is $132–$284 at list, 13–28× the workers it reviews. Experiment: two consecutive
   supervisor days at medium with the 3.17.2 actionable-idle metric and send-back counts as the score.
   Until then, change the registry's supervisor/taste `default_effort` to `high` so policy matches reality.
3. **Cut the supervisor's cache-write bill.** 53% of the 09-05 supervisor cost is 1-hour-TTL cache writes
   ($20/M on Fable 5.1, 80× the read price). That is a harness setting, not a model choice; a 5-minute
   TTL halves the write rate and is worth a one-day trial with the same metric.
4. **Light lane: give Haiku a thinking budget or delete the lane.** Haiku has no effort parameter, so
   `low` routes nothing meaningful; its one published coding score (73.3%) needs a 128K thinking budget.
   Either encode `thinking_budget` in the recipe and give it marker-tested chores, or route light at
   Luna/xhigh (which is what happens today, at $0.51).
5. **Track two ratios per lane weekly from the rollouts: output tokens per delivery and tool calls per
   delivery.** They are already in every rollout and they are what the vendors will not publish per
   effort. Luna's p50→p90 output spread (45K→74K) and Opus's 102K/78-call profile (fewer, bigger steps)
   are the first local intelligence-vs-efficiency signal we have; the send-back rate alone cannot
   separate "cheap and sloppy" from "expensive and careful".

What the data cannot answer: Sol/high vs Luna/xhigh vs Astra/high on identical tasks (the A/B in
recommendation 1), Fable medium vs high on identical supervision (recommendation 2), and any tool-calling
score for the OpenAI or Anthropic models (no τ²/BFCL figures are published for any of them).

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
  internal figures from `~/.codex/sessions/2026/08/20…09/06` rollouts (`token_count`, `turn_context`, tool
  calls), `~/.claude-daniel@petrastella.io/projects/…cas-src*` transcripts (per-message usage; all cache
  writes 1-hour TTL), `.cas/cas.db` task notes and `spawn_queue`, `.cas/logs/factory-session-*.log`;
  row-level data in `docs/factory/2026-09-06-model-lane-history.csv`, method in the `.md` beside it.
