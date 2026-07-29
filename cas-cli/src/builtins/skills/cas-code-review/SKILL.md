---
name: cas-code-review
description: Multi-persona code review orchestrator (Workflow-backed, Phase C — cas-b667). Thin wrapper around the `cas-code-review` Workflow: pre-fetches diff, calls Workflow (which handles Steps 1-4 internally: intent extraction, persona selection, size-gated parallel dispatch, deterministic merge), then routes results via Step 5 (mode dispatch + CAS integration). Use `mode=interactive` for the standard supervisor-driven path, `mode=report-only` for read-only scans, `mode=headless` for skill-to-skill calls. Factory workers MUST NOT invoke this skill pre-close — the supervisor owns review timing under the default `[code_review] owner = "supervisor"` configuration.
managed_by: cas
---

# cas-code-review — Workflow-backed multi-persona code review

**Architecture (Phase C, EPIC cas-b667; large-diff mode cas-33f1):** This skill is a thin wrapper. Steps 1-4 (intent extraction, persona selection, size-gated dispatch, merge) run inside the `cas-code-review` Workflow (`.claude/workflows/cas-code-review.js`). This skill handles Step 0 (tiny-diff bypass), Step 5 (mode routing + CAS integration), and pre-fetches the diff.

## Step 0: Tiny-diff bypass

Before any other work, check whether the diff warrants a full review:

1. `git diff --name-only <base_sha>..HEAD` — if every file matches a docs path (`*.md`, `docs/`) or test path (`tests/`, `*_test.rs`, `*.test.ts`, `*.spec.*`) → return a clean Allow envelope without calling the Workflow.
2. `git diff --shortstat <base_sha>..HEAD` — if fewer than 5 total lines changed AND no new files → return a clean Allow envelope.

Return shape: `{"residual": [], "pre_existing": [], "dropped": [], "mode": "<mode>", "skipped_reason": "...", "stats": {"dropped_findings": 0}}`.

## Steps 1-4: Resolve base SHA, pre-fetch, call Workflow

1. **Resolve `base_sha`** — if not supplied, use the Unit 3 helper (`crates/cas-store/src/code_review/base_sha.rs`: tries caller override → `GITHUB_BASE_REF` → `origin/HEAD` → common branches → `HEAD~1`).

2. **Pre-fetch diff inputs** (avoid agent output-token cost for large diffs):
   ```bash
   git diff <base_sha>..HEAD              # → diff_text
   git diff --name-only <base_sha>..HEAD  # → file_list
   git log --format=%B <base_sha>..HEAD   # → commit_log
   ```
   If `task_id` is known, also fetch task context: `mcp__cas__task action=show id=<task_id>` → `task_context` (title + description + acceptance criteria + notes).

3. **Call the Workflow:**
   ```
   Workflow({
     name: 'cas-code-review',
     args: {
       diff_text,     // full diff (pre-fetched)
       file_list,     // newline-separated paths (pre-fetched)
       base_sha,      // resolved SHA
       commit_log,    // git log output (for intent extraction)
       task_context,  // optional: task title+description+notes
       mode,          // current mode
       task_id,       // optional CAS task ID

       // optional large-diff threshold override:
       // large_diff_token_threshold, review_shard_token_threshold,
       // or shard_token_threshold (default: 12000 estimated tokens)
     }
   })
   ```

   The Workflow internally handles:
   - **Step 1** (intent): extracts a 2-3 line intent summary from commit_log / task_context
   - **Step 2** (selection): LLM-judged activation of conditional personas (security, performance, adversarial, fallow) plus optional diff-breadth activation of `gpt-5.6-sol:independent`
   - **Step 3** (dispatch): parallel persona dispatch through one shared schema shim. Correctness, testing, maintainability, project-standards, performance, adversarial, fallow, and the optional independent pass run through `codex exec -s read-only -m gpt-5.6-sol -c model_reasoning_effort=medium`; the shim supplies `REVIEWER_OUTPUT_SCHEMA` via `--output-schema`, parses the saved final JSON, applies two bounded schema-mismatch retries and one separately-budgeted timeout retry, and caps every process at 600 seconds. Security deliberately stays on Claude Opus as the cross-vendor high-signal lane. Diffs at or below the large-diff threshold keep the single full-diff dispatch shape. Diffs over the threshold are grouped into subsystem shards by module/concern plus one `interface-integrator` shard for cross-shard contracts. The Workflow validates that subsystem shard file lists cover the full changed-file union and logs missing, duplicate, or extra paths instead of silently dropping files. Docs-only and mechanical test shards use a reduced persona set; code shards keep the risk-weighted activated personas.
   - **Step 4** (merge): deterministic 7-step JS merge pipeline (Phase A validated, 30 unit tests)

   The Workflow returns `{ residual, pre_existing, dropped, intent_summary, activation, stats }`.
   When large-diff mode is enabled, `activation.sharding` records the threshold, estimated token count, shard IDs, shard file lists, routed personas, compact per-shard token counts, and coverage diagnostics. It does not include full shard diff bodies.

   `gpt-5.6-sol:independent` is opt-in for broad diffs only: 5+ changed files, 300+ changed lines, or an explicit Workflow arg (`gpt56_independent: true`; legacy `gpt55_independent: true` remains accepted). It never runs on tiny diffs because Step 0 returns before the Workflow. If codex is absent, auth fails, or the shared shim exhausts its timeout/schema retry budget, it returns `findings: []` with `skipped_reason`; activation records that as a skipped persona, distinct from a successful zero-finding review.

## Step 5: Mode-specific output

With the Workflow result in hand, branch on `mode`:

- **`autofix`** — feed merged output to Unit 7 (fixer sub-agent, max 2 rounds). Route residual non-`safe_auto` findings to CAS tasks (P0→0, P1→1, P2→2, P3→3; `advisory` never becomes a task). Any P0 hard-blocks the close; worker must fix or get supervisor override (R9). Legacy `owner=worker` path only.
- **`interactive`** — render findings severity-sorted, file+line anchored; offer bounded 2-round fix loop as an explicit choice; wait for human decision. Primary path under `owner=supervisor`.
- **`report-only`** — write merged envelope to `docs/reviews/<YYYY-MM-DD>-<short-ref>.md`. No edits, no task creation, no `task.close` side effects. Safe to run in parallel.
- **`headless`** — return merged envelope as structured text to the caller. No side effects.

In every mode, the output envelope contract is:

```
{
  "residual": Finding[],
  "pre_existing": Finding[],
  "dropped": DroppedFinding[],
  "mode": string,
  "intent_summary": string,
  "activation": object,
  "stats": { "dropped_findings": number, ... }
}
```

`dropped` retains every finding the deterministic merge cannot surface. Schema-invalid entries contain `{ reviewer, reason: "schema_validation_failed", validation_errors, finding }`; confidence-gated entries contain `{ reviewer, reason: "confidence_below_threshold", threshold, finding }`. `stats.dropped_findings` is exactly `dropped.length`. Consumers must render or persist these diagnostics rather than silently discarding them. `activation` records which personas ran and why, and `intent_summary` comes from the Workflow return value.

When the caller must pass the result into `task.close` via `code_review_findings`, its narrower wire envelope remains:

```
{ "residual": Finding[], "pre_existing": Finding[], "mode": string }
```

Each **Finding** requires: `title`, `severity`, `file`, `line`, `why_it_matters`, `autofix_class`, `owner`, `confidence`, `evidence`, `pre_existing`. Optional: `suggested_fix`, `requires_verification`. Full field rules live in [references/findings-schema.md](references/findings-schema.md). A malformed envelope is rejected with **all** missing Finding fields listed in one response — do not invent a partial shape by hand.

## Review ownership model

`[code_review] owner` in `.cas/config.toml`:

| `owner` | Worker behavior at close | Supervisor responsibility |
|---|---|---|
| `supervisor` **(default)** | Lightweight structural lint only; task → `pending_supervisor_review` | Run `/cas-code-review mode=interactive` at cherry-pick + EPIC→base merge |
| `worker` (opt-in legacy) | Full `autofix` pipeline inline; close blocks until done | None |

## Mode reference

| Mode | Edits files? | Creates tasks? | Gates close? | Fix loop |
|---|---|---|---|---|
| `autofix` (legacy `owner=worker`) | Yes via fixer on `safe_auto` | Yes, residual → CAS tasks | Yes, P0 hard-blocks | Bounded max 2 rounds |
| `interactive` | Only if user accepts loop | Only if user accepts | No | Bounded 2-round on consent |
| `report-only` | No | No | No | None |
| `headless` | No | No | No | None |

## Failure modes

- **Workflow errors** — if the Workflow fails (script error, schema validation exhausted), surface the error; do not fabricate findings.
- **Empty diff** — return clean Allow envelope (Step 0 bypass).
- **Base SHA resolution failure** — surface the error; do not fall back to a made-up base.
