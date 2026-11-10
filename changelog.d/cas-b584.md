### Fixed — Factory coordination

- **Slack claim refusals retain the owner** — When a Slack source belongs to a supervisor registered elsewhere, task creation shows the owning supervisor and the claim refusal. Violet also documents its source status audit error.
- **New tasks join the next release train** — After a train lands, standalone tasks join its successor while the prior train waits for release reconciliation.
