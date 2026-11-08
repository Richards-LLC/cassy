### Fixed — Task start

- **Reassigned tasks inherit their source branch safely** — Starting on the new worker's branch now rebinds only when both the previous local and origin tips are inherited. The rebind preserves task attribution and records the same branch history, audit note and ledger event as supervisor approval. Divergent or unavailable old tips retain the recorded source and the `rebind_branch` recovery action.
