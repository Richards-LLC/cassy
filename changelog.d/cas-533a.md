### Fixed — MCP

- **Read-only search deadlines** — Task evidence searches now report a non-mutating timeout instead of an unknown write. Timeout diagnostics retain search stage timings to distinguish index opening, task loading, task reconciliation and lookup.
