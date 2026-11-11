### Fixed — Release closeout

- **Reconcile with the installed release** — The installer verifies the new binary before running closeout. Failed older publications keep their own receipts and retry after newer installations, with bounded per-tick work and successful replay deduplication.
