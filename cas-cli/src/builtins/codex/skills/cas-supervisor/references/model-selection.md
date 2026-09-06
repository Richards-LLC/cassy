---
name: model-selection
description: Supervisor model/effort routing — match worker model tier to task complexity at breakdown, spawn, and escalation time.
managed_by: cas
---

# Model Selection — Matching Workers to Tasks

Pay for reasoning only where reasoning is the bottleneck. Every worker slot has three knobs — `cli`, `model`, `effort` — and the supervisor owns them: decide per task at breakdown, spawn the mix the backlog needs, escalate deliberately. Spawning everything at the session default wastes budget on chores and starves hard tasks of capability.

Routing is two stages. **Stage 1 — tier the task** by complexity; the tier is a stable property of the work. **Stage 2 — pick the registry lane** that fills that tier:

- **Light** is Claude Haiku 4.5 at the registry's default low effort: bounded chores, docs, and mechanical work still carry an explicit effort.
- **Standard** is Codex GPT-5.6 Luna at xhigh: the stock engineering floor for normal feature and bug work.
- **Taste** is Claude Fable 5.1 at medium: public surfaces, prompts, docs, naming, release notes, and general judgment are normal taste work, with Claude Opus 5/high as its loud fallback when Fable is unavailable.
- **Heavy** is Codex GPT-6 Astra at high: cross-cutting refactors, concurrency/lifecycle code, migrations, and critical-path work, with Codex GPT-5.6 Sol/high as its loud fallback.
- **OpenCode is route-specific** — its local and hosted Qwen lanes each require their
  own live receipt before production spawning. Never infer provider auth or effort
  support from the selector alone. The operator's default hosted lane is the explicit
  QwenCloud Token Plan route.

Luna remains xhigh-only; `max` and `ultra` are not Cassy effort values. The canonical policy is `crates/cas-factory/policy/lane-registry.toml`; the generated table below reflects its lane status.

## Registry route table

The route table below is generated from the embedded `cas-factory` registry. Keep the surrounding guidance human-authored; update policy in the registry and let the golden tests catch stale copies.

<!-- BEGIN GENERATED ROUTE TABLE: cas-factory lane registry -->
| Lane | Recipe | Provider | CLI | Model | Effort | Status | Fallback | Notes |
|---|---|---|---|---|---|---|---|---|
| `light` | `claude_haiku` | `anthropic` | `claude` | `claude-haiku-4-5-20251001` | `low` | `active` | `ordered candidates` |  |
| `standard` | `codex_luna` | `openai` | `codex` | `gpt-5.6-luna` | `xhigh` | `active` | `ordered candidates` |  |
| `taste` | `claude_fable` | `anthropic` | `claude` | `claude-fable-5-1` | `medium` | `active` | `fallback: claude_opus` |  |
| `heavy` | `codex_astra_high` | `openai` | `codex` | `gpt-6-astra` | `high` | `active` | `fallback: codex_sol` |  |
| `supervisor` | `claude_fable` | `anthropic` | `claude` | `claude-fable-5-1` | `medium` | `active` | `fallback: claude_opus` |  |
| `— (explicit only)` | `codex_astra` | `openai` | `codex` | `gpt-6-astra` | `medium` | `active` | `not lane-routed` | Heavy route; excluded from supervisor and taste after the observed 2026-09-05 stall holding finished workers and stopping epic drive. |
| `— (explicit only)` | `codex_terra` | `openai` | `codex` | `gpt-5.6-terra` | `xhigh` | `suspended` | `not lane-routed` | Standing operator suspension (2026-08-27) |
| `— (explicit only)` | `qwencloud_qwen` | `qwencloud` | `opencode` | `qwen3.8-max` | `medium` | `active` | `not lane-routed` | Receipt-gated by opencode-1.18.23-hosted-token-plan-2026-08-27; explicit recipe/model only |

Lane request mode: call `coordination spawn_workers` with `lane=<lane>`. The registry resolves the ordered candidates; any fallback selection is reported loudly as `fallback: <recipe> (primary <recipe> unavailable: <reason>)` in the spawn receipt and launch summary. Lanes marked `disabled` fail closed when their primary is unavailable.
<!-- END GENERATED ROUTE TABLE -->

Token-heavy read-only investigation belongs in a `cas-codex-exec` shell-out, not a worker and not your own context window.

### Taste lane

Use Claude Fable 5.1 at medium for architecture judgment, public decisions, rescue assessment, and independent challenge. Route safety-critical implementation through heavy. Taste falls back to Claude Opus 5/high when Fable is unavailable, and the spawn receipt names the fallback and primary-unavailable reason. Claude Opus remains supported for explicit Claude requests and as the taste/supervisor lane fallback. Claude Sonnet is not a normal worker lane and must not appear in copyable supervisor recipes.

### Capacity overlays

The registry's active lanes are the enforcement source for copyable routes. Provider capacity, authentication, and throughput may affect whether a lane can run, but availability facts do not create an undocumented fallback recipe. If a route is unavailable, report it and choose another active registry lane deliberately.

### OpenCode lane (route-specific conformance)

OpenCode supports three explicit OpenAI-compatible Qwen lanes through generated
primary agents and inline `cas` MCP config: `local/<model>` for the operator's
local server, `qwencloud/qwen3.8-max` for the operator's default QwenCloud Token
Plan lane, and `alibaba/qwen3.8-max` (or `alibaba-cn/qwen3.8-max`) for DashScope
pay-as-you-go. A lane is never inferred or used as fallback for another. Receipt
`opencode-1.18.23-hosted-token-plan-2026-08-27` validates only the Token Plan
route; local and Alibaba PAYG remain pending-conformance. Factory spawning fails
closed before queue insertion unless the selected route has its own matching
passing receipt. Do not persist keys in generated files or task receipts.

Token Plan fan-out follows the operator-declared plan tier: Lite permits 1–2,
Standard 3–4, and Pro 6–8 concurrent OpenCode agents. Warn or cap a spawn request
that exceeds the declared tier; do not scrape the operator console. A receipt may
carry the operator-declared tier as metadata when supplied.

- `cli=opencode model=local/qwen3.8` — local serving; endpoint reachability, model
  loading, and accepted effort variants come from the local operator preflight.
- `cli=opencode model=qwencloud/qwen3.8-max effort=low|medium|xhigh` — hosted
  Token Plan; the pinned endpoint is
  `https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1` and
  preflight requires `QWENCLOUD_TOKEN_PLAN_API_KEY` with the dedicated `sk-sp-`
  prefix. Its minimal preflight performs auth plus at most one tiny completion;
  it never lists models and never probes `/apps/anthropic`.
- `cli=opencode model=alibaba/qwen3.8-max effort=low|medium|xhigh` — hosted
  DashScope pay-as-you-go; preflight requires `DASHSCOPE_API_KEY` with `sk-` or
  `sk-ws-` prefix, endpoint reachability, and the selected model in `/models`.
  `alibaba-cn/...` selects the mainland endpoint. Token Plan and pay-as-you-go
  keys are lane-bound and a mismatch is refused before any network request.
- Each hosted Qwen lane currently accepts only `low`, `medium`, and `xhigh` for
  qwen3.8-max; `minimal` and `high` are rejected before OpenCode and are never
  silently remapped. Token Plan uses the OpenAI-compatible thinking body
  (`enable_thinking`); its effort table is independent of pay-as-you-go.
  The Token Plan route is supported by its OpenCode 1.18.23 live receipt, but
  still requires the dedicated key and bounded auth/answerability preflight on
  every spawn. Local and Alibaba PAYG remain `pending-conformance` and are
  refused before queue insertion.
- Every new conformance receipt records its explicit `route` and secret-free
  `serving_identity`; legacy receipts without these fields remain readable only as
  historical local-era fixtures.
- The OpenCode MCP server name `cas` yields `cas_task`, `cas_coordination`, and
  `cas_verification`; generated `cassy-worker`/`cassy-supervisor` prompts carry
  the role contract and remain process-local.

### Model slug table

| `cli=` | Accepted `model=` slugs | Notes |
|---|---|---|
| `codex` | `gpt-6-astra`, `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna` | Plain slugs only — `-codex`-suffixed slugs are rejected by the API, and bare `gpt-5.6` is invalid. Astra/high is the heavy route; Sol/high is its fallback; the medium Astra recipe remains explicit-only; Luna/xhigh is the standard route; Luna is the gpt-5.4-mini successor. |
| `claude` | any canonical `claude-*` id (e.g. `claude-fable-5-1`, `claude-opus-5`, `claude-haiku-4-5-20251001`, `claude-sonnet-5`) or the `opus`/`sonnet`/`haiku` aliases | Canonical IDs accept future numeric family/version releases and the CLI's optional `[1m]` context suffix; Haiku/low is the light lane; Fable/medium is the taste lane; Opus and Sonnet remain available for explicit Claude work; Opus is also the standard lane fallback. |
| `grok` | `grok-4.5`, `grok-4.6` | Provider capacity is not an active registry lane in this matrix; never invent `cli=cursor` or a fallback recipe. |
| `opencode` | `local/<model>`, `qwencloud/qwen3.8-max`, `alibaba/qwen3.8-max`, `alibaba-cn/qwen3.8-max` | Explicit local, Token Plan, or DashScope pay-as-you-go lane; per-lane conformance receipt required. Hosted auth/model availability are operator preflight inputs. |

### Stock fallback routes

When no factory configuration supplies a route, omitted controls resolve through
the harness stock fallback. Claude intentionally keeps the verified `opus`
alias as its stock model; this is a fallback route, not a registry lane.

```text
# Claude stock fallback
mcp__cs__coordination action=spawn_workers count=1 isolate=true cli=codex model=gpt-5.6-luna effort=xhigh
mcp__cs__coordination action=spawn_workers count=1 isolate=true cli=claude model=opus effort=high
```

### Effort vocabulary (Cassy-wide)

Accepted values: `minimal` \| `low` \| `medium` \| `high` \| `xhigh` (alias `x-high`).

How each backend receives them:

| Backend | Flag / config |
|---|---|
| Claude | `--effort <level>` |
| Codex | `--config model_reasoning_effort=<level>` |
| Grok | `--reasoning-effort <level>` |
| OpenCode | generated primary-agent `variant` (local: endpoint-specific; Token Plan/pay-as-you-go qwen3.8-max: `low`, `medium`, `xhigh`; Token Plan also pins `enable_thinking`) |

For non-Luna multi-step workers, `effort=high` is the ceiling. Luna is the exception: its only permitted Cassy effort is the current maximum, `xhigh`. The registry sets Haiku light to low, Fable taste to medium, and Astra heavy to high; do not use `max`/`ultra` until Cassy's vocabulary is extended and validated.

## Spawn recipes

The canonical copy-paste recipes are maintained once in [workflow.md](workflow.md#phase-2-coordinate). Use that generated block; this reference owns the registry policy and route table.

### OpenCode workers (route-specific conformance)

Use this recipe for the receipted OpenCode 1.18.23 Token Plan route:

```
mcp__cs__coordination action=spawn_workers count=1 isolate=true cli=opencode model=qwencloud/qwen3.8-max effort=medium worker_names="oc-ada"
```

The default hosted recipe requires `QWENCLOUD_TOKEN_PLAN_API_KEY` in the operator
environment and performs one bounded auth/answerability completion without model
discovery. Use `alibaba/qwen3.8-max` with `DASHSCOPE_API_KEY` for pay-as-you-go, or
`local/<model>` for a local server. Parameter table and field names:
[reference.md](reference.md#spawn_workers-parameters).

## Decision glossary

Use the registry lane as the default, then balance cost, intelligence, speed, and taste before spawning.

Glossary:

- **Cost** is budget spent per task (prefer $/task and tokens/task over list $/M tokens alone).
- **Intelligence** is how hard a problem the model can handle unsupervised: ambiguity, hidden coupling, long reasoning chains, and unfamiliar code.
- **Speed** is wall-clock and throughput: decode TPS, agent task wall time, and tokens burned per task.
- **Taste** is the quality of what ships: UI/UX judgment, API and SDK shape, naming, code style, prompts, docs, release notes, and error-message wording.

Taste-sensitive work uses the registry's Claude Fable 5.1/medium lane even when the diff is mechanically simple. Skill wording, supervisor guidance, release notes, public docs, API/SDK surfaces, and user-facing error text are not "light" just because the diff is small.

## Reading the task signals

Score each task while breaking down the EPIC:

- `task_type=chore`, docs-only, or `depth=light` → **light**
- Priority 0–1 on the critical path, or work touching 3+ modules/shared traits → **heavy**
- Taste, public-surface, or general-judgment work → **taste**
- Architecture, safety, rescue, or independent challenge → **taste** for the public decision, **heavy** for implementation risk
- Everything else → **standard**

Use the generated recipe block in [workflow.md](workflow.md#phase-2-coordinate) for the selected lane. Every command must carry explicit `cli=`, `model=`, and `effort=`; do not invent a fallback route outside the registry.

For every worker, `effort=high` is the ceiling except for the registry's Luna/xhigh standard route. `max` and `ultra` are not Cassy effort values. If a route is unavailable, report it and choose another active registry lane deliberately rather than silently changing the requested route.

## Workflow

1. **Tag at breakdown** — tasks default to standard; tag deviations with `labels="tier:light"` / `"tier:heavy"` and note non-obvious rationale in the task's `design` field.
2. **Spawn the mix** — count the lanes in the ready backlog and use the generated registry recipes above. Worker count may vary, but every `spawn_workers` command must retain the generated route's `cli`, `model`, and `effort`.
3. **Route by lane** — assign light, standard, taste, and heavy work to matching registry lanes. Use taste for public decisions and heavy for implementation risk.
4. **Escalate on failure** — after repeated rejection or verification failure, move deliberately to another active lane with the needed capability; never silently mutate an explicit recipe.
5. **Escalate on judgment** — the two-rejection rule is a floor, not a permission gate. Judge the output, not the price tag; use cheap lanes for information and drafts, then pay for what ships.
6. **De-escalate the tail** — when only light tasks remain, do not leave a heavy worker idle-burning; shut it down and let the light lane sweep the tail.

Explicit per-spawn parameters beat `.cas/config.toml` `[factory.defaults]` / `[[factory.workers]]` for that spawn only — check the project config before assuming what the floor actually is.
