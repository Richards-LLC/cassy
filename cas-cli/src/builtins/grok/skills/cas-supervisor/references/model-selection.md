---
name: model-selection
description: Supervisor model/effort routing — match worker model tier to task complexity at breakdown, spawn, and escalation time.
managed_by: cas
---

# Model Selection — Matching Workers to Tasks

Pay for reasoning only where reasoning is the bottleneck. Every worker slot has three knobs — `cli`, `model`, `effort` — and the supervisor owns them: decide per task at breakdown, spawn the mix the backlog needs, escalate deliberately. Spawning everything at the session default wastes budget on chores and starves hard tasks of capability.

Routing is two stages. **Stage 1 — tier the task** by complexity; the tier is a stable property of the work. **Stage 2 — pick the lane** that fills that tier:

- **Codex is the default matrix** — light work uses Terra/low, standard and taste/judgment work use Terra/high, and Sol/high is reserved for heavy and frontier work.
- **GPT-5.6 Terra high is the taste/judgment overlay** — route routine taste, public-surface, and general-judgment work to `cli=codex model=gpt-5.6-terra effort=high`.
- **Claude Opus is exceptional-only** — use it for architecture, safety, rescue, or independent challenge. Sonnet is not a normal worker lane.
- **Grok is a capacity overlay** — route to it while its credits/auth/throughput are healthy; fall back to the same Codex tier when they are not.

This default reflects the operator decision and published benchmarks: Terra is close on standard agentic work, while Sol's advantage is materially larger on frontier tasks. Change routing guidance only on independent, cross-vendor evidence (e.g. Artificial Analysis II/CAI leaderboards **plus** a live harness check) — never on a single failed spawn. A spawn failure from a mistyped slug tests the slug, not the model. Optional write-up: `docs/reports/2026-07-09-factory-model-matrix-recommendations.html` (rev 2).

## Tiers (Codex-first)

| Tier | Spawn parameters | Use for |
|---|---|---|
| **light** | `cli=codex model=gpt-5.6-terra effort=low` | Chores, docs, mechanical renames, config bumps, `depth=light` tasks, test backfill that mirrors existing patterns. |
| **standard** | `cli=codex model=gpt-5.6-terra effort=high` | Normal feature/bug work with a clear spec and bounded blast radius. The stock floor. |
| **heavy** | `cli=codex model=gpt-5.6-sol effort=high` | Cross-cutting refactors, concurrency/lifecycle code, migrations, gnarly debugging, P0/P1 critical-path units. |
| **frontier** | `cli=codex model=gpt-5.6-sol effort=high` | Architecture-defining units, high-blast-radius changes, tasks that already bounced twice. Sparingly — every frontier worker should map to named tasks. The heavy/frontier slug is `gpt-5.6-sol`; bare `gpt-5.6` is **not** a valid spawn recipe. |

Token-heavy read-only investigation belongs in a `cas-codex-exec` shell-out, not a worker and not your own context window.

### Taste/judgment lane (Codex GPT-5.6 Terra high)

Routine taste-sensitive output, public surfaces, API/SDK shape, naming, prompts, docs, release notes, error wording, and general judgment route to Codex GPT-5.6 Terra at high effort:

- **`cli=codex model=gpt-5.6-terra effort=high`** — routine taste / public-surface / general-judgment work. This is the normal lane that replaced routine Sonnet routing.

### Claude Opus lane (exceptional only)

Claude Sonnet is **not** a normal spawn lane and must not appear in copyable supervisor recipes. Keep Claude for exceptional cases only:

- **`cli=claude model=opus effort=high`** — architecture judgment, safety-critical changes, rescue of a stuck task, and independent / second-opinion challenge.

Max is still quota-limited capacity: keep `effort=high` as the ceiling on long worker loops (no `xhigh`/`max`), preserve explicit `cli`/`model`/`effort`, and fall back to the equivalent Codex tier when the Claude usage window is constrained.

### Grok lane (capacity routing — health-gated)

Grok is an optional credit/capacity route, not a required rung. Use it while healthy; fall back to the same-tier Codex rung when not.

- **`cli=grok model=grok-composer-2.5-fast effort=low`** — light / flash lane (Composer is a Grok model id, never `cli=cursor`); same-tier Codex fallback is `gpt-5.6-terra effort=low`.
- **`cli=grok model=grok-4.5 effort=medium|high`** — standard / heavy capacity; same-tier Codex fallback is `gpt-5.6-terra effort=high` for standard and `gpt-5.6-sol effort=high` for heavy.

Health check before routing to Grok: credits/quota available, auth valid, throughput healthy (`grok models` responds). If any is red, take the same-tier Codex rung instead.

### Model slug table

| `cli=` | Accepted `model=` slugs | Notes |
|---|---|---|
| `codex` | `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna` | Plain slugs only — `-codex`-suffixed slugs are rejected by the API, and bare `gpt-5.6` is invalid. Sol/high is the heavy/frontier route; Terra is the light/standard default and taste/judgment route; Luna is the gpt-5.4-mini successor and valid but not assigned a tier yet. The gpt-5.5/gpt-5.4 family is superseded: gpt-5.4 upgrades to Terra and gpt-5.4-mini to Luna. |
| `claude` | `opus` (full Anthropic ids also ok) | Supervisor docs only expose Opus for exceptional architecture/safety/rescue/challenge; Sonnet is not a normal worker lane. |
| `grok` | `grok-4.5`, `grok-composer-2.5-fast` | From live `grok models`. Composer is a **model id on the Grok harness** — never invent `cli=cursor`. |

### Effort vocabulary (CAS-wide)

Accepted values: `minimal` \| `low` \| `medium` \| `high` \| `xhigh` (alias `x-high`).

How each backend receives them:

| Backend | Flag / config |
|---|---|
| Claude | `--effort <level>` |
| Codex | `--config model_reasoning_effort=<level>` |
| Grok | `--reasoning-effort <level>` |

For multi-step workers, `effort=high` is the ceiling — do not reach for `xhigh`/`max` on long agent loops (overthink + cost). Codex tiers use Terra/low for light, Terra/high for standard and taste, and Sol/high for heavy/frontier.

## Spawn cookbook (all three harnesses)

Copy-paste `spawn_workers` recipes. Examples below use this harness's coordination tool prefix. Worker `cli=`/`model=`/`effort=` are independent of which harness the supervisor runs on — `cli=codex` works from Claude, Codex, or Grok supervisors alike.

### Codex workers (default matrix)

```
# light / bulk
cas__coordination action=spawn_workers count=2 isolate=true cli=codex model=gpt-5.6-terra effort=low

# standard floor
cas__coordination action=spawn_workers count=2 isolate=true cli=codex model=gpt-5.6-terra effort=high

# heavy
cas__coordination action=spawn_workers count=1 isolate=true cli=codex model=gpt-5.6-sol effort=high worker_names="hv-ada"

# frontier — exact slug gpt-5.6-sol
cas__coordination action=spawn_workers count=1 isolate=true cli=codex model=gpt-5.6-sol effort=high worker_names="fr-ada"
```

### Taste / judgment workers (Codex GPT-5.6 Terra high)

```
# taste / public-surface / general-judgment work
cas__coordination action=spawn_workers count=1 isolate=true cli=codex model=gpt-5.6-terra effort=high worker_names="tj-ada"
```

### Claude Opus workers (exceptional: architecture / safety / rescue / challenge)

```
# exceptional architecture / safety / rescue / independent challenge
cas__coordination action=spawn_workers count=1 isolate=true cli=claude model=opus effort=high worker_names="op-ada"
```

### Grok workers (capacity — use while credits/auth/throughput healthy)

```
# light / flash — Composer model id on cli=grok
cas__coordination action=spawn_workers count=1 isolate=true cli=grok model=grok-composer-2.5-fast effort=low worker_names="lt-ada"

# standard / heavy capacity
cas__coordination action=spawn_workers count=2 isolate=true cli=grok model=grok-4.5 effort=medium
cas__coordination action=spawn_workers count=1 isolate=true cli=grok model=grok-4.5 effort=high worker_names="gh-ada"
```

Parameter table and field names: [reference.md](reference.md#spawn_workers-parameters).

## Routing Axes

Use tier labels as defaults, then check four axes before spawning:

| Tier | Cost | Intelligence | Speed | Taste |
|---|---|---|---|---|
| **light** | Lowest agent $/task (Codex gpt-5.6-terra low; Grok Composer when capacity-routed) | Sufficient for well-bounded mechanical work | Highest — low wall time / flash lane | Low: fine for renames and internal scaffolding; review public surfaces |
| **standard** | Low — Codex gpt-5.6-terra high is the cost-efficient floor | High for bounded engineering; default for most factory work | High throughput on sustained agent loops | Low-to-mid: fine for internal code; review user-facing prose |
| **heavy** | Codex gpt-5.6-sol high; Opus/Grok only when exception/capacity says so | High for messy codebases, lifecycle bugs, multi-module judgment | Strong multi-step agent loops; slower than light on tiny tasks | Mid: good default for critical-path code; use GPT-5.6 Terra high for taste |
| **frontier** | Highest — reserve for quality/risk that justifies it | Highest ceiling (Codex gpt-5.6-sol high; Claude Opus only for exceptional architecture / challenge) | Slowest / most expensive agent loops | High: taste-sensitive output that must land cleanly |

Glossary:

- **Cost** is budget spent per task (prefer $/task and tokens/task over list $/M tokens alone). Codex is the default lane; Claude Opus is exceptional and quota-limited; Grok is credit-gated capacity.
- **Intelligence** is how hard a problem the model can handle unsupervised: ambiguity, hidden coupling, long reasoning chains, and unfamiliar code.
- **Speed** is wall-clock and throughput: decode TPS, agent task wall time, and tokens burned per task.
- **Taste** is the quality of what ships: UI/UX judgment, API and SDK shape, naming, code style, prompts, docs, release notes, and error-message wording.

Taste-sensitive work routes to Codex GPT-5.6 Terra at high effort even when the task is mechanically simple. Skill wording, supervisor guidance, release notes, public docs, API/SDK surfaces, and user-facing error text are not "light" just because the diff is small — start those on `cli=codex model=gpt-5.6-terra effort=high`, not Sonnet and not the cheapest lane.

## Reading the task signals

Score each task while breaking down the EPIC:

- `task_type=chore`, docs-only, or `depth=light` → **light**
- Spike whose question is architectural ("which design holds at 10x?") → **heavy** or **frontier**; mechanical spikes ("does the API return X?") → **light** or **standard**
- Priority 0–1 AND on the critical path → at least **heavy**
- Touches 3+ modules, shared traits/schemas, or unwind/panic/locking behavior → **heavy**
- You would read the diff twice yourself before merging → **frontier**
- Taste, public-surface, or general-judgment work → `cli=codex model=gpt-5.6-terra effort=high`
- Architecture, safety, rescue, or independent challenge in play → the **Claude Opus exceptional lane**
- Everything else → **standard** (the default is the default for a reason)

Use the task tier to select the model/effort pair: `gpt-5.6-terra effort=low` for light work, `gpt-5.6-terra effort=high` for standard and taste/general judgment, and `gpt-5.6-sol effort=high` for heavy/frontier reasoning. Claude Opus remains exceptional; use Grok for capacity relief.

For every worker, `effort=high` is the ceiling. `xhigh`/`max` increase per-step reasoning, not step count or run length; on hard multi-step work they tend to overthink each move, produce heavier diffs, and multiply cost. Escalate the model tier or split the task before raising effort above `high`.

## Workflow

1. **Tag at breakdown** — tasks default to standard; tag deviations with a label: `labels="tier:light"` / `"tier:heavy"` / `"tier:frontier"`. Note non-obvious tier rationale (and any fit/capacity lane choice) in the task's `design` field.
2. **Spawn the mix** — count tiers in the ready backlog, then issue one `spawn_workers` call per tier (a call's parameters apply to every worker in that call):
   ```
   # two standard workers (floor)
   cas__coordination action=spawn_workers count=2 isolate=true cli=codex model=gpt-5.6-terra effort=high

   # one light worker for chores
   cas__coordination action=spawn_workers count=1 isolate=true cli=codex model=gpt-5.6-terra effort=low worker_names="lt-ada"

   # one heavy worker for tier:heavy tasks
   cas__coordination action=spawn_workers count=1 isolate=true cli=codex model=gpt-5.6-sol effort=high worker_names="hv-ada"
   ```
   Every `spawn_workers` call MUST include explicit `cli=`, `model=`, and `effort=`.
   Relying on omitted fields is a fallback path that emits an acknowledgement
   warning, not an approved supervisor workflow.
   Tiers change the fleet's composition, not its size — worker-count strategy (3–4 max, sized by independent file groups) still applies.
3. **Route by tier and lane** — assign `tier:*`-labelled tasks to a matching worker; standard tasks go to anyone at that tier. Send taste/general judgment to `gpt-5.6-terra effort=high`, exceptional architecture/safety/rescue/challenge to Opus, and capacity overflow to Grok while healthy. Name heavier workers so routing stays legible (`hv-*`, `fr-*`, `tj-*`, `op-*`).
4. **Escalate on failure** — a task rejected or verification-failed twice moves up one tier: move from Terra to Sol/high for heavy/frontier reasoning, or use Opus for an exceptional architecture/safety/rescue/challenge case. Don't re-run the same task on the same tier hoping for different output.
5. **Escalate on judgment** — the two-rejection rule is a floor, not a permission gate. If a cheaper worker's draft gathers facts but misses the quality bar, escalate before verification fails. Judge the output, not the price tag; use cheap tiers for information and drafts, then pay for what ships. Cost is a tiebreaker only.

### Escalation ladder (Codex-first, with fit/capacity overlays)

```
light     codex  model=gpt-5.6-terra effort=low
→ standard  codex  model=gpt-5.6-terra effort=high
  → heavy     codex  model=gpt-5.6-sol effort=high
    → frontier  codex  model=gpt-5.6-sol effort=high   # exact slug; +reasoning ceiling
  taste lane:   codex  model=gpt-5.6-terra effort=high # taste / public surface / judgment
  exception:    claude model=opus effort=high           # architecture / safety / rescue / challenge
  capacity:     grok   model=grok-composer-2.5-fast effort=low | grok-4.5 effort=medium|high (health-gated)
```

- Reserve `gpt-5.6-sol effort=high` for heavy/frontier work before assuming you need another vendor.
- Taste, public-surface, and general-judgment work can jump straight to `cli=codex model=gpt-5.6-terra effort=high` even if the diff is small.
- Claude Sonnet is not a normal spawn lane; Opus is reserved for architecture, safety, rescue, and independent challenge.
- Route to Grok only while its credits/auth/throughput are healthy; otherwise take the same-tier Codex rung.
6. **De-escalate the tail** — when only light tasks remain, don't leave a heavy/frontier worker idle-burning; shut it down and let light workers sweep the tail.

Explicit per-spawn parameters beat `.cas/config.toml` `[factory.defaults]` / `[[factory.workers]]` for that spawn only — check the project config before assuming what the floor actually is.
