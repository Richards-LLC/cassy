### Fixed — MCP tool schema size

- **The verification tool's schema fits its size budget again** — Was: the longer `verification_type` wording pushed the published tool list to 59,089 bytes, over its 59,000-byte budget. Now: the same guidance (omit it for a batch QA receipt; `release` is the review verdict) in shorter text, and the list is back under budget.
