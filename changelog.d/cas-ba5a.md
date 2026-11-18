### Fixed — Developer tooling

- **Mock response scenarios pass test-shape lint** — A Rust loop variable such as `for source in ["a", "b"]` that iterates only string literals is no longer treated as source text inside that loop's own body, so a mock API error response keyed on it is not flagged as a text pin. Code outside the loop, and a body that rebinds the name with `let` or closure parameters, keep every existing check.
