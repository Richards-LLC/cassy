### Fixed — Batch QA receipts

- **batch_receipt says which record it wrote** — Was: the `verification_type` parameter read "batch_receipt: release", so a supervisor recorded a batch QA receipt as a release code-review verdict, which then blocked closes; the committed line did not say which record was written. Now: the parameter says to omit it for a batch QA receipt and that release is only the code-review verdict, and the committed line names the record (Batch QA receipt or Release verdict).
