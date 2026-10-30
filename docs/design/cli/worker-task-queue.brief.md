# Worker task queue concept

Audience: a factory supervisor assigning several tasks to one worker.
Decision: see the current task and pending order, then replace, reorder or clear only pending work.
Hierarchy: queue owner and revision, current task, pending order, latest deferred or skipped decision.
Constraints: use the existing plain-text MCP status surface, no ANSI or TTY-only state; keep previous delivery refs and single-task ownership visible.
Machine output: the factory MCP response retains its existing content/text envelope; `expected_generation` accepts the reported revision as a replacement precondition.

design: DESIGN.md@b43940e43

## Critique

Hierarchy 4/5: current and pending work are separately named. Fit 4/5: existing factory names and task IDs remain copyable. Craft 4/5: queue decisions name the affected task and cause. This is an MCP text surface with fixture evidence, not a new terminal command or TTY renderer.
