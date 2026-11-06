### Fixed — Factory

- **Preserve concurrent work and close intent** — A refused reviewed landing rolls back only its own merge. Matching active verification dispatches retain no-delivery close declarations, and held automatic closes name the supervisor action needed.
- **Avoid idle worker-directory lookups** — Peer polling loads Cloud worker authority and task ancestry only for received local-worker mail, while retaining fresh authorization before delivery.
- **Recognize trailing task tags** — Task ownership includes trailing parentheses and brackets before squash PR numbers.

### Fixed — MCP

- **Retry transient drain deferrals** — The stable front retries the same installed backend after a drain timeout while continuing to suppress failed or incompatible images until the next install.

### Fixed — Search

- **Accept word-edge apostrophes** — Possessives and leading year abbreviations search as text while single-quoted phrases remain supported.
