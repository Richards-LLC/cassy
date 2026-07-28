# BUG: stock worker defaults (`gpt-5.5`, `sonnet`) contradict the routing policy CAS itself ships

**Filed:** 2026-07-28
**Reporter:** supervisor `wild-condor-51`, factory session `Penguinz-witty-viper-34` (project: Penguinz, host: soundwave)
**Severity:** High — silent, and it routes real work to models the shipped policy says are retired.

## Summary

`spawn_workers` with an omitted `model=` resolves to **`gpt-5.5`** for Codex workers and **`sonnet`** for Claude workers. Both contradict `model-selection.md`, which CAS ships as a `managed_by: cas` builtin, and which states the Codex matrix is exactly `gpt-5.6-sol` at low/medium/high.

The contradiction is enforced in the same repo. `cas-cli/src/builtins.rs:3611` asserts that no line of the shipped routing doc may contain `model=gpt-5.5` or `model=sonnet`:

```rust
assert!(
    !line.contains("model=gpt-5.5") && !line.contains("model=sonnet"),
    "{label}:{} contains a disallowed worker route: {line:?}",
```

So the documentation is test-guarded against exactly the two values the runtime falls back to.

## Observed

```
mcp__cas__coordination action=spawn_workers count=1 task_id=cas-2f5d isolate=true
-> Queued spawn request for 1 worker(s) (request ID: 50)
   Worker spec: codex model=gpt-5.5 effort=medium
   Warning: spawn_workers should include explicit model= and effort=; omitted
   fields were resolved to safe worker defaults for this request.
```

The warning fires, but it does not name the resolved model as a policy deviation — it reads as routine hygiene advice. A supervisor who does not already know the policy has no signal that they just got a retired model.

## Root cause

Two constants, both reachable when the caller omits `model=`:

| Site | Current | Should be |
|---|---|---|
| `cas-cli/src/config/settings.rs:613` | `pub const STOCK_WORKER_MODEL: &str = "gpt-5.5";` | `"gpt-5.6-sol"` |
| `cas-cli/src/mcp/tools/service/factory_ops.rs:64` | `SupervisorCli::Claude => "sonnet"` | `"claude-opus-5"` (see note) |

`default_worker_model_for_cli()` in `factory_ops.rs:62-69` is the resolution point; `STOCK_WORKER_MODEL` feeds its Codex arm.

Note on the Claude arm: Sonnet is being retired for price/performance reasons. The operator's current policy is a two-lane split — `codex/gpt-5.6-sol/medium` as the default worker, `claude/claude-opus-5/medium` as the taste/finesse worker. Since the Codex auto-upgrade at `factory_ops.rs:132` already redirects an unconfigured Claude default to Codex, the Claude arm is only reachable when Claude is explicitly configured — so it should resolve to the lane a deliberate Claude choice implies, which is Opus 5, not a retired model.

## Why this is not just cosmetic

`.cas/config.toml` does not have to declare `[factory.defaults]` for this to bite — neither the global `~/.cas/config.toml` nor the Penguinz project config declares any factory section, so the binary constant IS the effective floor on this machine. Every supervisor that omits `model=` gets `gpt-5.5` regardless of what the shipped doc says. The warning does not stop the spawn.

## Suggested fix

1. Point `STOCK_WORKER_MODEL` at `gpt-5.6-sol` and the Claude arm at `claude-opus-5`.
2. Make the omitted-`model=` warning name the resolved model and state whether it matches current policy, rather than the generic hygiene sentence. "resolved to gpt-5.6-sol (current default lane)" is actionable; "resolved to safe worker defaults" is not.
3. Add a test asserting `STOCK_WORKER_MODEL` is one of the slugs the `builtins.rs` routing-doc assertions permit, so the constant and the doc can never drift apart again. That single test is the actual fix — the constants will go stale again otherwise.

## Related

Companion request: `FEATURE-code-review-personas-off-claude-only-model-enum.md` (persona fleet routing after the Sonnet retirement).
