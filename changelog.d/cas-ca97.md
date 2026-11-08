### Fixed — Batch QA receipts

- **A batch receipt is refused for a ledger the close gate would reject** — Was: `verification action=batch_receipt` recorded a receipt on a ledger that `cas qa ledger check` rejected (duplicate headline counts, label mismatch), so the epic close later failed at verification. Now: batch_receipt runs the same ledger check against the ledger owner task first and refuses with its diagnostics before any receipt is written.
