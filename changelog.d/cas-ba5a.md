### Fixed — Developer tooling

- **Mock response scenarios pass test-shape lint** — A Rust loop variable such as `for source in ["a", "b"]` that holds only string literals and is bound nowhere else is no longer treated as source text, so a mock API error response keyed on it is not flagged as a text pin. A `source` bound to file text keeps every existing check.
