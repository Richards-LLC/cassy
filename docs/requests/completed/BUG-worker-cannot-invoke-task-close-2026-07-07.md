# BUG: factory worker unable to invoke `mcp__cas__task action=close` — MCP tools load via ToolSearch but are not callable

**From:** petra-stella-cloud team (supervisor session, 2026-07-07)
**Severity:** Medium-high — breaks the "workers own their closes" invariant; forces supervisor escape-hatch closes
**Component:** factory worker toolchain / MCP dispatch in worker sessions

## What happened

Worker `lt-defects` (cli=claude, model=haiku, effort=low, spawned via `spawn_workers` on cas 2.27.0 / 9ebc844) completed task cas-d01c end-to-end: implemented, committed (39114fc + rework 6cad785), pushed, build+test green, supervisor-approved and merged into the epic branch. When instructed to close its own task, it reported:

> The `mcp__cas__task action=close id=cas-d01c` command cannot be invoked through the available interfaces in this worker session:
> - MCP tools (mcp__cas__task) are loaded via ToolSearch but not directly callable as functions
> - Cassy CLI available at /home/pippenz/.local/bin/cas but does not have a 'task' subcommand for closing

Notable: the same worker successfully used `mcp__cas__task action=mine` earlier in its session (its first check-in referenced it), so tool availability appears to have degraded or the close call specifically fails. Also possible: a haiku-tier worker mishandling the ToolSearch deferred-schema flow (load schema → call), but the CLI-fallback claim ("no task subcommand") suggests it exhausted plausible paths.

The supervisor had to close via the escape hatch (`bypass_code_review=true`, audit note on cas-d01c), which the hard rules reserve for dead workers — a live, responsive worker should never need it.

## Impact

- Violates worker-owned close: supervisor closes skew audit trails and bypass the worker-side close gates (verification-jail, review envelope).
- If widespread on low-tier workers, every task ends in a supervisor escape-hatch close.

## Suggested investigation

1. Reproduce: spawn a haiku/low claude worker, have it complete a trivial task, observe whether `mcp__cas__task action=close` is callable late in the session (vs `mine` early).
2. Check whether the worker system prompt / harness makes the deferred-ToolSearch flow explicit enough for low-tier models (load schema THEN call — a haiku worker may need this spelled out or the task tool pre-loaded, not deferred).
3. Consider pre-loading `mcp__cas__task` (non-deferred) in worker sessions — it is guaranteed-needed by every worker for mine/start/notes/close.
4. CLI fallback: if `cas task close` (or equivalent) exists, make sure workers know the exact syntax; if it doesn't, that's a missing escape path.

## Related reports (same session)

- BUG-spawn-workers-inherits-supervisor-model-2026-07-07.md
- BUG-finished-notification-while-close-rejected-2026-07-07.md
