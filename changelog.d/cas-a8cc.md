### Fixed — Batch QA

- **A failing ledger no longer records a Batch QA PASS** — Was: a structurally
  valid ledger with FAIL cells produced an immutable PASS receipt. Now: the
  receipt is refused and names each failed cell; the ledger artifact itself
  stays published as evidence.
