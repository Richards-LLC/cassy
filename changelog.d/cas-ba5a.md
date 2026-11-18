### Fixed — Developer tooling

- **Mock response scenarios pass test-shape lint** — A scenario selector named `source` no longer makes a mock API error response look like a source-text pin. The exemption covers the protocol error field's own line inside the JSON response input call; source-text and prose assertions retain their checks.
