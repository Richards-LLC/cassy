### Fixed — Release automation

- **Exhausted installation notices** — Stop retrying a post-release notice after ten attempts, preserve its failed receipt and send one failure record to the release's owning supervisor. Reporting retries reuse that record without repeating release reconciliation.
