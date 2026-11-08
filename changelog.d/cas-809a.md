### Fixed — Batch QA status

- **qa_status on an epic names the child receipts that cover it** — Was: `verification action=qa_status` on a release-train epic said "no valid staging receipt", although batch_receipt refuses an epic as self-review so it could never be recorded. Now: the epic's status lists each child delivery with the receipt that covers it, and no longer asks for a receipt that cannot exist.
