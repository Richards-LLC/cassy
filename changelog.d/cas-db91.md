### Fixed — Rebound delivery branches

- **Close checks name the rebound branch even before its first commit** — After a supervisor rebinds a task to a new lane (for example `factory/<worker>-<task>-r2`), the close checks printed at task start, the publication check and every other close gate now measure that lane, even while it is still empty. Before, an empty rebound lane fell back to the old per-task branch that still held the earlier build (GH #1698).
