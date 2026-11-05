# Model Selection — Matching Workers to Tasks

Pay for reasoning only where reasoning is the bottleneck. You own each worker's `cli`, `model` and `effort`: tier each task at breakdown, spawn the mix the backlog needs, and escalate deliberately. This file is the one copy of the lane matrix.

- **Light** is Codex GPT-6 Luna at xhigh: bounded chores and mechanical non-public docs.
- **Standard** is Codex GPT-6.1 Sol at high: the floor for normal feature and bug work.
- **Taste** is Claude Opus 5.5 at high: public surfaces, prompts, docs, naming, release notes and general judgment.
- **Heavy** is Claude Opus 5.5 at high: cross-cutting refactors, concurrency/lifecycle code, migrations and critical-path work.
- **Supervisor** is Claude Opus 5.5 at high.
- **OpenCode is route-specific**: each Qwen route needs its own live receipt; see [OpenCode lane](#opencode-lane-route-specific-conformance).

Pass `lane=<light|standard|taste|heavy>` (preferred) or a complete explicit `cli=`/`model=`/`effort=` recipe, never both. Fallbacks fire only in lane mode, and the receipt names them; an explicit recipe runs as written or fails.

## Registry route table

The table is generated from the embedded `cas-factory` registry; change policy in the registry, not here.

<!-- BEGIN GENERATED ROUTE TABLE: cas-factory lane registry -->
| Lane | Recipe | Provider | CLI | Model | Effort | Status | Fallback | Notes |
|---|---|---|---|---|---|---|---|---|
| `light` | `codex_luna_6` | `openai` | `codex` | `gpt-6-luna` | `xhigh` | `active` | `fallback: claude_opus_5_5_low` |  |
| `standard` | `codex_sol_6_1` | `openai` | `codex` | `gpt-6.1-sol` | `high` | `active` | `fallback: codex_sol_6` |  |
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

Token-heavy read-only investigation belongs in a `cas-codex-exec` shell-out, not a worker or your own context.

## Reading the task signals

Score each task while breaking down the EPIC:

- `task_type=chore` or mechanical, non-public docs → **light**
- Priority 0–1 on the critical path, or work touching 3+ modules or shared traits → **heavy**
- Public docs, skills, prompts and other taste or general-judgment work → **taste**, even when the diff is small
- Architecture, safety, rescue or independent challenge → **taste** for the decision, **heavy** for the implementation
- Everything else → **standard**

`effort=high` is the ceiling except for the light lane's Luna/xhigh and an explicit operator request for `max` on a recipe that lists it, after a task already failed at `high`/`xhigh`. Claude Sonnet is not a worker lane. If a route is unavailable, report it and choose another active registry lane deliberately; never invent a fallback.

## Workflow

1. **Tag at breakdown:** tasks default to standard; tag deviations with `labels="tier:light"`, `"tier:taste"` or `"tier:heavy"` and note non-obvious rationale in `design`.
2. **Spawn the mix:** one `lane=<lane>` spawn, or complete explicit recipe, per lane the ready backlog needs.
3. **Escalate on failure or judgment:** after repeated rejection, move deliberately to a lane with the needed capability. Judge the output, not the price: use cheap lanes for drafts and pay for what ships.
4. **De-escalate the tail:** when only light tasks remain, shut down idle heavy workers.

Explicit spawn parameters beat `.cas/config.toml` `[factory.defaults]` and `[[factory.workers]]` for that spawn only.

### Stock fallback routes

Omitted controls resolve through factory configuration, then `[llm.worker]` (or `[llm]`), then the harness stock fallback; the receipt names each field's source.

```text
# Codex stock fallback
factory action=spawn_workers count=1 isolate=true cli=codex model=gpt-6.1-sol effort=high
# Claude stock fallback
factory action=spawn_workers count=1 isolate=true cli=claude model=opus effort=high
```

### OpenCode lane (route-specific conformance)

Read this only when spawning with `cli=opencode`. Each route needs its own passing receipt and is never a fallback for another:

- `cli=opencode model=qwencloud/qwen3.8-max effort=low|medium|xhigh`: the hosted QwenCloud Token Plan; needs `QWENCLOUD_TOKEN_PLAN_API_KEY`.
- `cli=opencode model=alibaba/qwen3.8-max effort=low|medium|xhigh`: DashScope pay-as-you-go (`alibaba-cn/...` for the mainland endpoint); needs `DASHSCOPE_API_KEY`.
- `cli=opencode model=local/<model>`: the operator's local server.

Keep Token Plan fan-out within the operator's declared tier (Lite 1–2, Standard 3–4, Pro 6–8 agents). Never persist keys in generated files or receipts.

## Spawn recipes

Copy lane recipes from the generated block in [workflow.md](workflow.md#phase-2-coordinate). The receipted OpenCode route:

```text
factory action=spawn_workers count=1 isolate=true cli=opencode model=qwencloud/qwen3.8-max effort=medium worker_names="oc-ada"
```
