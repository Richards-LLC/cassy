### Fixed — Factory and artifacts

- **Host retention** — Sweep expired, uncited task artifacts across registered projects automatically on idle maintenance and disk pressure; report reclaimed bytes and preserved paths. Build outputs remain regenerable even when cited; live owners, unpublished source and durable artifact references retain their protections.
- **Cold artifact lists** — Include synced publication references alongside local records and explicitly report a partial inventory. An empty local cache no longer claims that no artifacts were published.
- **Candidate build guidance** — Give supervisors the direct candidate-build command when the worker queue or sandbox is unavailable, including on macOS.
