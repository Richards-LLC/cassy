## Core Concepts

- **Gate registry** - factory checks have stable codes across gate, guard, watchdog, notice and CI families. `cas gates list` measures the live registry from source declarations. MCP tool, hook and release/pre-push refusals carry their `[CODE]`; `cas gates report --since <tag>` counts recorded refusals, overrides and false-positive declarations per code. Uncoded refusals remain countable. Workflow C checks are counted and coded; their per-code facts follow separately.
