# FEATURE: route cas-code-review personas to `gpt-5.6-sol` medium (they are stranded on a Claude-only model enum)

**Filed:** 2026-07-28
**Reporter:** supervisor `wild-condor-51`, factory session `Penguinz-witty-viper-34` (project: Penguinz, host: soundwave)
**Severity:** Medium — blocks completion of the Sonnet retirement; currently forces a quality/cost choice nobody wants.

## Problem

With Sonnet retired, the `cas-code-review` persona fleet has nowhere good to go.

All seven personas (`correctness`, `security`, `adversarial`, `performance`, `maintainability`, `testing`, `project-standards`) plus the `fallow` adapter dispatch through the Workflow `agent()` call with a `model:` parameter whose accepted values are the **Claude-only** enum — `sonnet | opus | haiku | fable`. `gpt-5.6-sol` is not expressible there.

That leaves two bad options:

- **Haiku** — cheap, but a real step down for `security`, `correctness`, and `adversarial`, which are the personas whose false negatives actually cost something.
- **Opus** — good, but this is a 7-way parallel fan-out on *every* review. Paying taste-lane prices for bounded schema-constrained analysis is exactly the misrouting the new policy is meant to eliminate.

The operator's stated policy is `codex/gpt-5.6-sol/medium` as the default worker lane. Persona review is squarely default-lane work: bounded, spec'd, schema-constrained analysis against a diff. It is not taste work and it is not architecture work.

## The precedent already exists

The workflow already reaches Codex for exactly one persona. `builtins.rs:2764` and `:2888` assert these marker strings:

```
"gpt-5.5:independent",
"codex exec -s read-only -m gpt-5.5",
```

So `codex exec -s read-only` is already a shipped, test-guarded persona transport. This request is to generalise that path to the rest of the fleet, not to invent a new one. `-s read-only` is also a strictly better sandbox for review than a full-tool Agent — a reviewer has no business writing.

## The actual engineering cost

This is not a find-and-replace, and it should not be filed as one.

`agent(prompt, {schema: REVIEWER_OUTPUT_SCHEMA})` forces a `StructuredOutput` tool call and validates at the tool-call layer, so the model is automatically retried on schema mismatch. `codex exec` returns raw text. Moving the personas to Codex means writing, once:

- a parse + validate + retry shim against `REVIEWER_OUTPUT_SCHEMA`
- a bounded retry policy (schema mismatch is not the same failure as a timeout)
- a per-persona timeout, since 7 concurrent `codex exec` processes is a different resource profile from 7 in-harness agents

There are five `model:` dispatch sites in `.claude/workflows/cas-code-review.js` (currently at lines 747, 802, 833, 844, 856). The shim should be written once and shared across all of them.

## Recommendation

Route the persona fleet to `codex exec -s read-only -m gpt-5.6-sol` at medium reasoning effort, **with one deliberate exception**: keep at least one high-signal persona — `adversarial` or `security` — on a Claude model.

Rationale for the exception: the fleet's value comes from independent perspectives. The current shape is an Opus orchestrator dispatching Sonnet personas with one Codex `:independent` reviewer — cross-vendor diversity was clearly intentional. Moving 100% of the personas to Codex inverts that and leaves the whole review monovendor, which quietly deletes the property the `:independent` persona was added to provide. Keep the split, just flip which side is the minority.

## Blocking detail for whoever picks this up

`.claude/skills/cas-code-review/**` and `.claude/workflows/cas-code-review.js` are `managed_by: cas` builtins generated from `builtins.rs`. Editing them in a consuming project (e.g. Penguinz) is overwritten on the next sync. The change must be made in `builtins.rs` and its content assertions updated together — specifically the `gpt-5.5` marker assertions at `builtins.rs:2764`, `:2888`, and `:2544`, which will fail the moment the persona transport slug changes.

## Related

Companion request: `BUG-stock-worker-defaults-contradict-shipped-model-routing-policy.md`.
