### Fixed — Release

- **Two-machine merges find the parking machine's evidence and parks** — Was: a delivery parked on one machine could stay "in progress" on the merge owner after a proof-bearing sync (GH #2105), and its nested captures (`proof/…`) and top-level logs never reached the owner, so the merge refused with a missing PASS capture (GH #2079). Now: restoring synced proof no longer outranks the park, and every park lists its evidence files by task-relative path so the owner restores each one where the ledger cites it, without a manual copy or a new gate.
