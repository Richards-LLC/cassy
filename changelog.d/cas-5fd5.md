### Fixed — MCP

- **Finish promised background tool work before replacement** — Backend idle and retirement now wait for tool futures that outlive their response, so an installed update defers instead of terminating the remaining work.

### Fixed — Factory

- **Close unassigned no-delivery tasks on the first attempt** — Verification close intent uses the assignee persisted by that close.
- **Retain worker mail across claim observation races** — Clock-leading renewals keep valid epic membership, and missing receipt scope leaves mail pending for redelivery while invalid identities and intents remain rejected.
