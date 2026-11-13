## Core Concepts

- **Runtime fixture mutation** — Test-shape lint accepts an immediate same-path formatted Rust fixture rewrite when the read value has no other consumer. Assertions and comparisons of source text still require behavioral coverage instead, so real Git/MCP regressions can change disposable compiled inputs without a pin or bypass.
