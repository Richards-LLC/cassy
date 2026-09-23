# Model market matrix — 2026-09-23 (GPT-6 Sol/Luna and Claude Opus 5.5 launch)

Audience: operator. Type: research input for the model-lane rubric decision. Nothing here
changes `crates/cas-factory/policy/lane-registry.toml`; placements below are candidates for
discussion, gated on operator approval (decision recorded 2026-09-23).

Launches on 2026-09-22: OpenAI GPT-6 Sol (`gpt-6-sol`) and GPT-6 Luna (`gpt-6-luna`); Anthropic
Claude Opus 5.5 (`claude-opus-5-5`). Anthropic says Sonnet 5.5 and Haiku 5.5 follow "in the coming
weeks". Installed Codex CLI 0.156.0 already lists `gpt-6-sol` (low–ultra) and `gpt-6-luna`
(low–max) in `~/.codex/models_cache.json`.

## List prices (USD per million tokens, standard tier)

| Model | Input | Cache read | Cache write | Output | Source |
| --- | ---: | ---: | ---: | ---: | --- |
| GPT-6 Astra | 10.00 | 1.00 | 12.50 | 50.00 | [OpenAI pricing](https://developers.openai.com/api/docs/pricing) |
| GPT-6 Sol | 2.00 | 0.20 | 2.50 | 10.00 | OpenAI pricing |
| GPT-6 Luna | 0.10 | 0.01 | 0.125 | 0.50 | OpenAI pricing |
| GPT-5.6 Sol (promo through 2026-11-21) | 4.00 | 0.40 | 5.00 | 20.00 | OpenAI pricing |
| GPT-5.6 Luna | 0.20 | 0.02 | — | 1.20 | [AA comparison](https://artificialanalysis.ai/models/comparisons/gpt-6-sol-vs-gpt-5-6-luna-high) |
| Claude Fable 5.1 | 10.00 | 0.25 | 12.50 (5m) / 20 (1h) | 50.00 | [Anthropic pricing](https://docs.anthropic.com/en/docs/about-claude/pricing) |
| Claude Opus 5.5 | 4.00 | 0.20 | 5.00 (5m) / 8 (1h) | 20.00 | Anthropic pricing |
| Claude Opus 5 | 5.00 | 0.50 | 6.25 (5m) / 10 (1h) | 25.00 | Anthropic pricing |
| Claude Sonnet 5 | 2.00 | 0.20 | 2.50 (5m) / 4 (1h) | 10.00 | Anthropic pricing |
| Claude Haiku 4.5 | 1.00 | 0.10 | 1.25 (5m) / 2 (1h) | 5.00 | Anthropic pricing |

## General intelligence vs cost per task (Artificial Analysis Intelligence Index v4.3.2)

| Model · effort | Index | Cost / task | Output tokens / task | Source |
| --- | ---: | ---: | ---: | --- |
| Claude Opus 5.5 · max | 57.6 | $5.98 | 119,200 | [FinOps LLM](https://finopsllm.com/research/claude-opus-5-5-cost-model), [AA](https://artificialanalysis.ai/articles/claude-opus-5-5) |
| Claude Opus 5.5 · high | 53.6 | $1.82 | 35,600 | FinOps LLM |
| Claude Fable 5.1 · max | 53.4 | $7.63 | 78,100 | FinOps LLM |
| GPT-6 Astra · max | 52.7 | $3.26 | 27,200 | FinOps LLM |
| Claude Opus 5.5 · medium | 51.2 | $1.34 | 25,700 | FinOps LLM |
| Claude Fable 5.1 · high | 51.2 | $3.91 | 38,100 | FinOps LLM |
| Claude Opus 5 · max | 50.8 | $5.86 | 72,500 | FinOps LLM |
| Claude Opus 5 · high | 48.1 | $3.61 | 46,200 | FinOps LLM |
| GPT-6 Sol · max | 47.5 | $1.06 | 31,200 | FinOps LLM, [AA](https://artificialanalysis.ai/articles/gpt-6-sol-and-luna-push-the-cost-efficiency-frontier) |
| GPT-6 Astra · low | 45.8 | $0.82 | 4,400 | FinOps LLM |
| GPT-6 Sol · xhigh | 44.1 | $0.53 | 16,000 | FinOps LLM |
| GPT-6 Sol · medium | 39.8 | $0.25 | 6,500 | FinOps LLM |
| Claude Sonnet 5 · max | 38 | $5.09 | — | [AA](https://artificialanalysis.ai/models/comparisons) |
| GPT-6 Luna · max | 37 | $0.07 | 51,000 | [AA release](https://artificialanalysis.ai/models/releases/gpt-6-luna) |
| GPT-5.6 Luna · high | 32 | $0.04 | — | AA comparison |
| Claude Haiku 4.5 · reasoning | 17 | $0.21 | — | AA comparison |

Pareto read (FinOps LLM, same data): GPT-6 Sol is the cheapest route to any score up to 47.5;
above that Claude Opus 5.5 is cheapest at every level. Every Astra setting above low, every
Opus 5 setting and every Fable 5.1 setting costs more than a Sol or Opus 5.5 setting that
scores the same or higher. Sonnet 5 and Haiku 4.5 are dominated by GPT-6 Sol/Luna.

## Coding agents in their real harness (AA Coding Agent Index v1.5)

DeepSWE v1.1 + Terminal-Bench 4.0 + SWE-Atlas-QnA, pass@1 over three attempts, pay-per-token cost.
Source: [AA Claude Code vs Codex](https://artificialanalysis.ai/agents/coding-agents/comparisons/claude-code-vs-codex).
Opus 5.5 has no Coding Agent Index entry yet.

| Harness · model · effort | Index | DeepSWE | TB 4.0 | SWE-Atlas | Cost / task | Time / task |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Claude Code · Fable 5.1 · max | 62 | 64% | 58% | 65% | $12.39 | 34.8m |
| Codex · GPT-6 Astra · max | 62 | 68% | 56% | 62% | $7.47 | 29.4m |
| Claude Code · Opus 5 · max | 60 | 63% | 55% | 62% | $10.79 | 41.9m |
| Codex · GPT-6 Sol · max | 57 | 69% | 43% | 58% | $2.99 | 22.3m |
| Codex · GPT-5.6 Sol · max | 55 | 72% | 37% | 54% | $6.35 | 20.6m |
| Codex · GPT-5.6 Luna · max | 43 | 66% | 15% | 49% | $0.44 | 23.5m |
| Codex · GPT-6 Luna · max | 41 | 64% | 15% | 44% | $0.18 | 21.4m |

Vendor-reported (Anthropic, Opus 5.5 at max unless noted): Terminal-Bench 4.0 66.4% (xhigh) vs
Fable 5.1 55.8%, GPT-6 Astra 57.9%, Opus 5 52.3%; FrontierCode v1.1 54.4% vs Astra 53.3%;
at default (medium) effort 54.6% on FrontierCode "for about a fifth of the cost per task" of
Astra. Source: [Anthropic](https://www.anthropic.com/claude-opus-5-5). Treat vendor numbers as
hypotheses.

## Caveats

- GPT-6 Luna regresses 2 points on the Coding Agent Index vs GPT-5.6 Luna; both GPT-6 models
  drop ~75–100 Elo on GDPval-AA (shorter deliverables that omit rubric elements) — relevant to
  report and document work (AA article).
- Opus 5.5 thinking cannot be disabled; it uses 1.6× Opus 5's output tokens at max.
- Benchmarks price pay-per-token; the factory runs on subscriptions, so cost is a shadow price.
- Our own measured window (docs/factory/2026-09-23-model-lane-rubric-refresh.md): GPT-5.6 Luna
  xhigh 315 deliveries, 23.81% send-backs, $0.89/task; Opus 5.5 high 20 deliveries, 5.00%,
  $7.66/task; Astra high 47, 29.79%, $16.71/task; no GPT-6 Sol/Luna rows yet.

## GPT-6 Sol and Luna by effort level

AA Intelligence Index v4.3.2 per-model pages (artificialanalysis.ai/models/gpt-6-{sol,luna}[-effort]),
read 2026-09-23. Cost per task marked `~` is estimated from output-token volume, anchored on the
published costs of the same model; unmarked costs are published by AA / FinOps LLM.

| Model · effort | Index | Cost / task | Output tokens (whole index run) | Output speed |
| --- | ---: | ---: | ---: | ---: |
| GPT-6 Sol · max | 48 | $1.06 | 77M | 126 t/s |
| GPT-6 Sol · xhigh | 44 | $0.53 | 40M | 136 t/s |
| GPT-6 Sol · high | 43 | ~$0.34 | 25M | 138 t/s |
| GPT-6 Sol · medium | 40 | $0.25 | 16M | 114 t/s |
| GPT-6 Sol · low | 34 | $0.13 | 8.9M | 118 t/s |
| GPT-6 Luna · max | 37 | $0.07 | 150M | 157 t/s |
| GPT-6 Luna · xhigh | 34 | ~$0.034 | 68M | 153 t/s |
| GPT-6 Luna · high | 32 | ~$0.024 | 47M | 127 t/s |
| GPT-6 Luna · medium | 29 | ~$0.014 | 28M | 143 t/s |
| GPT-6 Luna · low | 21 | $0.0045 | 8.1M | 152 t/s |

Coding Agent Index (Codex harness) is published at max only: Sol 57 at $2.99/task, Luna 41 at
$0.18/task; GPT-5.6 Luna max 43 at $0.44.
