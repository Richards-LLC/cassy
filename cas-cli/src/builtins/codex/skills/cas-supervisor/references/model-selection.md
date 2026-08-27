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
- **Taste** is Claude Opus 5 at high: public surfaces, prompts, docs, naming, release notes, and general judgment are normal Opus work, not a special-case escape hatch.
- **Heavy** is Codex GPT-5.6 Sol at high: cross-cutting refactors, concurrency/lifecycle code, migrations, and critical-path work.

Terra is a **standing suspension**: `gpt-5.6-terra` is documented for compatibility, but it is not an active lane and must never be spawned. Luna remains xhigh-only; `max` and `ultra` are not Cassy effort values.

## Registry route table

The route table below is generated from the embedded `cas-factory` registry. Keep the surrounding guidance human-authored; update policy in the registry and let the golden tests catch stale copies.

<!-- BEGIN GENERATED ROUTE TABLE: cas-factory lane registry -->
| Lane | Recipe | Provider | CLI | Model | Effort | Status | Fallback |
|---|---|---|---|---|---|---|---|
| `light` | `claude_haiku` | `anthropic` | `claude` | `haiku-4.5` | `low` | `active` | `ordered candidates` |
| `standard` | `codex_luna` | `openai` | `codex` | `gpt-5.6-luna` | `xhigh` | `active` | `ordered candidates` |
| `taste` | `claude_opus` | `anthropic` | `claude` | `opus-5` | `high` | `active` | `disabled` |
| `heavy` | `codex_sol` | `openai` | `codex` | `gpt-5.6-sol` | `high` | `active` | `ordered candidates` |

Lane request mode: `coordination action=spawn_workers lane=<lane>`. The registry resolves the ordered candidates; any non-primary selection is reported as a warning with the selected recipe and reason. Lanes marked `disabled` fail closed when their primary is unavailable.
<!-- END GENERATED ROUTE TABLE -->

Token-heavy read-only investigation belongs in a `cas-codex-exec` shell-out, not a worker and not your own context window.

### Taste/judgment lane (Claude Opus 5 high)

Taste-sensitive output is a first-class registry lane: route public surfaces, API/SDK shape, naming, prompts, docs, release notes, error wording, and general judgment to Claude Opus 5 at high effort. This is normal taste work, not a special-case escape hatch.

### Claude Opus lane (taste plus exceptional architecture)

Claude Opus is the registry's taste lane and is also the right fit for architecture judgment, safety-critical changes, rescue of a stuck task, and independent challenge. Claude Sonnet is not a normal worker lane and must not appear in copyable supervisor recipes. Keep effort at high for these multi-step workers.

### Capacity overlays

The registry's active lanes are the enforcement source for copyable routes. Provider capacity, authentication, and throughput may affect whether a lane can run, but availability facts do not create an undocumented fallback recipe. If a route is unavailable, report it and choose another active registry lane deliberately.

### Model slug table

| `cli=` | Accepted `model=` slugs | Notes |
|---|---|---|
| `codex` | `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna` | Plain slugs only — `-codex`-suffixed slugs are rejected by the API, and bare `gpt-5.6` is invalid. Sol/high is the heavy route; Luna/xhigh is the standard route; **Terra is standing-suspended and has no active lane**. Luna is the gpt-5.4-mini successor. |
| `claude` | `haiku-4.5`, `opus-5` (full Anthropic ids also ok) | Haiku/low is the light lane; Opus/high is the taste lane and also serves exceptional architecture/safety/rescue/challenge. Sonnet is not a normal worker lane. |
| `grok` | `grok-4.5`, `grok-composer-2.5-fast` | Provider capacity is not an active registry lane in this matrix; never invent `cli=cursor` or a fallback recipe. |

### Effort vocabulary (Cassy-wide)

Accepted values: `minimal` \| `low` \| `medium` \| `high` \| `xhigh` (alias `x-high`).

How each backend receives them:

| Backend | Flag / config |
|---|---|
| Claude | `--effort <level>` |
| Codex | `--config model_reasoning_effort=<level>` |
| Grok | `--reasoning-effort <level>` |

For non-Luna multi-step workers, `effort=high` is the ceiling. Luna is the exception: its only permitted Cassy effort is the current maximum, `xhigh`. The registry sets Haiku light to low, Opus taste to high, and Sol heavy to high; do not use `max`/`ultra` until Cassy's vocabulary is extended and validated.

## Spawn cookbook (all three harnesses)

Copy-paste `spawn_workers` recipes generated from the active registry lanes. The tool prefix changes with the supervisor harness, but every command pins the registry's `cli`, `model`, and `effort`.

<!-- BEGIN GENERATED SPAWN RECIPES: cas-factory lane registry -->
Copy-paste commands generated from the registry; every recipe pins `cli`, `model`, and `effort`:

```text
# light — recipe claude_haiku
mcp__cs__coordination action=spawn_workers count=1 isolate=true cli=claude model=haiku-4.5 effort=low

# standard — recipe codex_luna
mcp__cs__coordination action=spawn_workers count=1 isolate=true cli=codex model=gpt-5.6-luna effort=xhigh

# taste — recipe claude_opus
mcp__cs__coordination action=spawn_workers count=1 isolate=true cli=claude model=opus-5 effort=high

# heavy — recipe codex_sol
mcp__cs__coordination action=spawn_workers count=1 isolate=true cli=codex model=gpt-5.6-sol effort=high

```
<!-- END GENERATED SPAWN RECIPES -->

Parameter table and field names: [reference.md](reference.md#spawn_workers-parameters).

## Routing Axes

Use the registry lane as the default, then check four axes before spawning:

| Lane | Cost | Intelligence | Speed | Taste |
|---|---|---|---|---|
| **light** | Lowest agent cost | Sufficient for bounded mechanical work | Highest throughput | Review public surfaces before shipping |
| **standard** | Default engineering cost | High for clear feature/bug work | High throughput | Review user-facing prose |
| **taste** | Higher cost justified by public quality | High judgment ceiling | Strong multi-step output | First-class: docs, prompts, naming, API shape, and release notes |
| **heavy** | Critical-path cost | High for messy, cross-cutting work | Slower but deliberate | Strong engineering judgment |

Glossary:

- **Cost** is budget spent per task (prefer $/task and tokens/task over list $/M tokens alone).
- **Intelligence** is how hard a problem the model can handle unsupervised: ambiguity, hidden coupling, long reasoning chains, and unfamiliar code.
- **Speed** is wall-clock and throughput: decode TPS, agent task wall time, and tokens burned per task.
- **Taste** is the quality of what ships: UI/UX judgment, API and SDK shape, naming, code style, prompts, docs, release notes, and error-message wording.

Taste-sensitive work uses the registry's Claude Opus 5/high lane even when the diff is mechanically simple. Skill wording, supervisor guidance, release notes, public docs, API/SDK surfaces, and user-facing error text are not "light" just because the diff is small.

## Reading the task signals

Score each task while breaking down the EPIC:

- `task_type=chore`, docs-only, or `depth=light` → **light**
- Priority 0–1 on the critical path, or work touching 3+ modules/shared traits → **heavy**
- Taste, public-surface, or general-judgment work → **taste**
- Architecture, safety, rescue, or independent challenge → **taste** for the public decision, **heavy** for implementation risk
- Everything else → **standard**

Use the generated recipe block above for the selected lane. Every command must carry explicit `cli=`, `model=`, and `effort=`; do not invent a fallback route outside the registry. Terra is standing-suspended.

For every worker, `effort=high` is the ceiling except for the registry's Luna/xhigh standard route. `max` and `ultra` are not Cassy effort values. If a route is unavailable, report it and choose another active registry lane deliberately rather than silently changing the requested route.

## Workflow

1. **Tag at breakdown** — tasks default to standard; tag deviations with `labels="tier:light"` / `"tier:heavy"` and note non-obvious rationale in the task's `design` field.
2. **Spawn the mix** — count the lanes in the ready backlog and use the generated registry recipes above. Worker count may vary, but every `spawn_workers` command must retain the generated route's `cli`, `model`, and `effort`.
3. **Route by lane** — assign light, standard, taste, and heavy work to matching registry lanes. Opus is the normal taste lane and also fits exceptional architecture, safety, rescue, and challenge.
4. **Escalate on failure** — after repeated rejection or verification failure, move deliberately to another active lane with the needed capability; never silently mutate an explicit recipe.
5. **Escalate on judgment** — the two-rejection rule is a floor, not a permission gate. Judge the output, not the price tag; use cheap lanes for information and drafts, then pay for what ships.
6. **De-escalate the tail** — when only light tasks remain, do not leave a heavy worker idle-burning; shut it down and let the light lane sweep the tail.

Explicit per-spawn parameters beat `.cas/config.toml` `[factory.defaults]` / `[[factory.workers]]` for that spawn only — check the project config before assuming what the floor actually is.
