# Model Selection — Matching Workers to Tasks

Pay for reasoning only where reasoning is the bottleneck. Every worker slot has three knobs — `cli`, `model`, `effort` — and the supervisor owns them: decide per task at breakdown, spawn the mix the backlog needs, escalate deliberately. Spawning everything at the session default wastes budget on chores and starves hard tasks of capability.

This file is the one authoritative copy of the lane matrix; the skill body and other references point here.

Routing is two stages. **Stage 1 — tier the task** by complexity; the tier is a stable property of the work. **Stage 2 — pick the registry lane** that fills that tier:

- **Light** is Codex GPT-6 Luna at xhigh, with Claude Opus 5.5/low as fallback: bounded chores, mechanical non-public docs, and other mechanical work.
- **Standard** is Codex GPT-6 Sol at medium, with Codex GPT-6 Luna/xhigh as fallback: the stock engineering floor for normal feature and bug work.
- **Supervisor** is Claude Opus 5.5 at high, with Claude Fable 5.1/high as fallback.
- **Taste** is Claude Opus 5.5 at high: public surfaces, prompts, docs, naming, release notes, and general judgment are normal taste work, with Claude Opus 5/high as its loud fallback when Opus 5.5 is unavailable.
- **Heavy** is Claude Opus 5.5 at high: cross-cutting refactors, concurrency/lifecycle code, migrations, and critical-path work, with Codex GPT-6 Astra/high as its loud fallback.
- **OpenCode is route-specific** — each Qwen route needs its own live receipt; see [OpenCode lane](#opencode-lane-route-specific-conformance).

**How to spawn a lane.** Pass `lane=<light|standard|taste|heavy>` (preferred) or a complete explicit `cli=`/`model=`/`effort=` recipe to force one model — never both. The fallbacks above fire only in lane mode; an explicit recipe runs as written or fails. The generated table below reflects the registry's lane status.

## Registry route table

The route table below is generated from the embedded `cas-factory` registry. Keep the surrounding guidance human-authored; update policy in the registry and let the golden tests catch stale copies.

<!-- BEGIN GENERATED ROUTE TABLE: cas-factory lane registry -->
| Lane | Recipe | Provider | CLI | Model | Effort | Status | Fallback | Notes |
|---|---|---|---|---|---|---|---|---|
| `light` | `codex_luna_6` | `openai` | `codex` | `gpt-6-luna` | `xhigh` | `active` | `fallback: claude_opus_5_5_low` |  |
| `standard` | `codex_sol_6` | `openai` | `codex` | `gpt-6-sol` | `medium` | `active` | `fallback: codex_luna_6` |  |
| `taste` | `claude_opus_5_5` | `anthropic` | `claude` | `claude-opus-5-5` | `high` | `active` | `fallback: claude_opus` |  |
| `heavy` | `claude_opus_5_5` | `anthropic` | `claude` | `claude-opus-5-5` | `high` | `active` | `fallback: codex_astra_high` |  |
| `supervisor` | `claude_opus_5_5` | `anthropic` | `claude` | `claude-opus-5-5` | `high` | `active` | `fallback: claude_fable_high` |  |
| `— (explicit only)` | `claude_fable` | `anthropic` | `claude` | `claude-fable-5-1` | `medium` | `active` | `not lane-routed` |  |
| `— (explicit only)` | `codex_astra` | `openai` | `codex` | `gpt-6-astra` | `medium` | `active` | `not lane-routed` | Heavy route; excluded from supervisor and taste after the observed 2026-09-05 stall holding finished workers and stopping epic drive. |
| `— (explicit only)` | `codex_luna` | `openai` | `codex` | `gpt-5.6-luna` | `xhigh` | `active` | `not lane-routed` |  |
| `— (explicit only)` | `codex_sol` | `openai` | `codex` | `gpt-5.6-sol` | `high` | `active` | `not lane-routed` |  |
| `— (explicit only)` | `codex_terra` | `openai` | `codex` | `gpt-5.6-terra` | `xhigh` | `suspended` | `not lane-routed` | Standing operator suspension (2026-08-27) |
| `— (explicit only)` | `qwencloud_qwen` | `qwencloud` | `opencode` | `qwen3.8-max` | `medium` | `active` | `not lane-routed` | Receipt-gated by opencode-1.18.23-hosted-token-plan-2026-08-27; explicit recipe/model only |

Lane request mode: call the `factory` tool's `spawn_workers` action with `lane=<lane>`. The registry resolves the ordered candidates; any fallback selection is reported loudly as `fallback: <recipe> (primary <recipe> unavailable: <reason>)` in the spawn receipt and launch summary. Lanes marked `disabled` fail closed when their primary is unavailable.
<!-- END GENERATED ROUTE TABLE -->

Token-heavy read-only investigation belongs in a `cas-codex-exec` shell-out, not a worker and not your own context window.

### Taste lane

Use taste for architecture judgment, public decisions, rescue assessment, and independent challenge. Route safety-critical implementation through heavy. In lane mode the spawn receipt names any fallback and the primary-unavailable reason. Claude Sonnet is not a normal worker lane and must not appear in copyable supervisor recipes.

### Capacity overlays

The registry's active lanes are the enforcement source for copyable routes. Provider capacity, authentication, and throughput may affect whether a lane can run, but availability facts do not create an undocumented fallback recipe. If a route is unavailable, report it and choose another active registry lane deliberately.

### OpenCode lane (route-specific conformance)

Read this section only when spawning with `cli=opencode`. OpenCode has three
explicit Qwen routes, never inferred or used as a fallback for one another:

- `cli=opencode model=qwencloud/qwen3.8-max effort=low|medium|xhigh` — the
  operator's default hosted QwenCloud Token Plan route; needs
  `QWENCLOUD_TOKEN_PLAN_API_KEY` (`sk-sp-` prefix). Validated by receipt
  `opencode-1.18.23-hosted-token-plan-2026-08-27`.
- `cli=opencode model=alibaba/qwen3.8-max effort=low|medium|xhigh` — DashScope
  pay-as-you-go (`alibaba-cn/...` for the mainland endpoint); needs
  `DASHSCOPE_API_KEY`. Pending conformance.
- `cli=opencode model=local/<model>` — the operator's local server; effort
  variants come from its preflight. Pending conformance.

A route without its own passing receipt is refused before queue insertion, and a
key for the wrong route is refused before any network request. Hosted routes
accept only `low`, `medium`, and `xhigh`; other efforts are rejected, never
remapped. Token Plan fan-out follows the operator-declared tier (Lite 1–2,
Standard 3–4, Pro 6–8 agents): warn or cap beyond it and do not scrape the
operator console. Never persist keys in generated files or receipts. The
OpenCode MCP tools are `cas_task`, `cas_coordination`, and `cas_verification`.

### Model slug table

| `cli=` | Accepted `model=` slugs | Notes |
|---|---|---|
| `codex` | `gpt-6-astra`, `gpt-6-sol`, `gpt-6-luna`, `gpt-5.6-sol`, `gpt-5.6-luna` | Plain slugs only — `-codex`-suffixed slugs are rejected by the API, and bare `gpt-5.6` is invalid. |
| `claude` | any canonical `claude-*` id (e.g. `claude-fable-5-1`, `claude-opus-5-5`, `claude-opus-5`, `claude-sonnet-5`) or the `opus`/`sonnet` aliases | Canonical IDs accept future numeric family/version releases and the CLI's optional `[1m]` context suffix; Haiku requests are rejected. |
| `grok` | `grok-4.5`, `grok-4.6` | Provider capacity is not an active registry lane in this matrix; never invent `cli=cursor` or a fallback recipe. |
| `opencode` | `local/<model>`, `qwencloud/qwen3.8-max`, `alibaba/qwen3.8-max`, `alibaba-cn/qwen3.8-max` | Per-route receipt required; see [OpenCode lane](#opencode-lane-route-specific-conformance). |

### Stock fallback routes

When no factory configuration supplies a route, omitted controls resolve through
the harness stock fallback. Claude intentionally keeps the verified `opus`
alias as its stock model; this is a fallback route, not a registry lane.

```text
# Codex stock fallback
cas__factory action=spawn_workers count=1 isolate=true cli=codex model=gpt-6-sol effort=medium
# Claude stock fallback
cas__factory action=spawn_workers count=1 isolate=true cli=claude model=opus effort=high
```

### Effort vocabulary (Cassy-wide)

Accepted values: `minimal` \| `low` \| `medium` \| `high` \| `xhigh` (alias `x-high`) \| `max`.

How each backend receives them:

| Backend | Flag / config |
|---|---|
| Claude | `--effort <level>` |
| Codex | `--config model_reasoning_effort=<level>` |
| Grok | `--reasoning-effort <level>` |
| OpenCode | generated primary-agent `variant` (local: endpoint-specific; Token Plan/pay-as-you-go qwen3.8-max: `low`, `medium`, `xhigh`; Token Plan also pins `enable_thinking`) |

For multi-step workers, choose the lane default. `ultra` is not a Cassy effort value.

### `max` effort (explicit request only)

`max` is never a lane default; use it only when a task already failed at `high`/`xhigh` and the operator asked for it. Accepted where the recipe lists it: Claude Fable 5.1 and Opus 5 (Claude effort doc, https://platform.claude.com/docs/en/build-with-claude/effort), Codex GPT-6 Astra, GPT-6 Sol, GPT-6 Luna, and GPT-5.6 Sol (`codex-rs/protocol/src/openai_models.rs`). Refused elsewhere:

| Recipe | `max` |
|---|---|
| `claude_opus_5_5` (Claude Opus 5.5) | rejected — `low`/`high` only |
| `codex_luna` (GPT-5.6 Luna) | rejected — `xhigh` only |
| OpenCode Qwen lanes | rejected |

## Spawn recipes

The canonical copy-paste recipes are maintained once in [workflow.md](workflow.md#phase-2-coordinate). Use that generated block; this reference owns the registry policy and route table.

### OpenCode workers (route-specific conformance)

Use this recipe for the receipted OpenCode 1.18.23 Token Plan route:

```
cas__factory action=spawn_workers count=1 isolate=true cli=opencode model=qwencloud/qwen3.8-max effort=medium worker_names="oc-ada"
```

Route requirements are in [OpenCode lane](#opencode-lane-route-specific-conformance); parameter table in
[reference.md](reference.md).

## Decision glossary

Use the registry lane as the default, then balance cost, intelligence, speed, and taste before spawning.

Glossary:

- **Cost** is budget spent per task (prefer $/task and tokens/task over list $/M tokens alone).
- **Intelligence** is how hard a problem the model can handle unsupervised: ambiguity, hidden coupling, long reasoning chains, and unfamiliar code.
- **Speed** is wall-clock and throughput: decode TPS, agent task wall time, and tokens burned per task.
- **Taste** is the quality of what ships: UI/UX judgment, API and SDK shape, naming, code style, prompts, docs, release notes, and error-message wording.

Taste-sensitive work uses the registry's Claude Opus 5.5/high lane even when the diff is mechanically simple. Skill wording, supervisor guidance, release notes, public docs, API/SDK surfaces, and user-facing error text are not "light" just because the diff is small.

## Reading the task signals

Score each task while breaking down the EPIC:

- `task_type=chore` or mechanical, non-public docs → **light**
- Priority 0–1 on the critical path, or work touching 3+ modules/shared traits → **heavy**
- Public docs, skills, prompts, and other taste, public-surface, or general-judgment work → **taste**
- Architecture, safety, rescue, or independent challenge → **taste** for the public decision, **heavy** for implementation risk
- Everything else → **standard**

Spawn the selected lane with `lane=<lane>`, or copy its explicit recipe from [workflow.md](workflow.md#phase-2-coordinate) to force one model; do not invent a fallback route outside the registry.

For every worker, `effort=high` is the ceiling except for the registry's Luna/xhigh light route and an explicit operator request for `max` on a recipe that lists it. If a route is unavailable, report it and choose another active registry lane deliberately rather than silently changing the requested route.

## Workflow

1. **Tag at breakdown** — tasks default to standard; tag deviations with `labels="tier:light"` / `"tier:heavy"` and note non-obvious rationale in the task's `design` field.
2. **Spawn the mix** — count the lanes in the ready backlog and spawn each with `lane=<lane>` or its complete explicit recipe, never a partial one.
3. **Route by lane** — assign light, standard, taste, and heavy work to matching registry lanes. Use taste for public decisions and heavy for implementation risk.
4. **Escalate on failure** — after repeated rejection or verification failure, move deliberately to another active lane with the needed capability; never silently mutate an explicit recipe.
5. **Escalate on judgment** — the two-rejection rule is a floor, not a permission gate. Judge the output, not the price tag; use cheap lanes for information and drafts, then pay for what ships.
6. **De-escalate the tail** — when only light tasks remain, do not leave a heavy worker idle-burning; shut it down and let the light lane sweep the tail.

Explicit per-spawn parameters beat `.cas/config.toml` `[factory.defaults]` / `[[factory.workers]]` for that spawn only — check the project config before assuming what the floor actually is.
