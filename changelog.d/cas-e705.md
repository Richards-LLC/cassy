### Fixed — Historical delivery recovery

- **Prescribed recovery closes no longer loop after reviewer errors** — A live
  supervisor can replay the historical recovery command for the exact recorded
  delivery already integrated into its target, including an earlier reviewer
  ERROR. Recovery retains the original error and dispatch, records the audit
  decision, and creates no pre-merge approval. Unintegrated deliveries and
  unauthorized callers retain their existing refusals.
