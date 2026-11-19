### Fixed — Release verification

- **Tag verdicts reuse reviewed delivery drops** — A published tag now accepts the same validated supersession review as release closeout. The stored review must cover the exact delivery, its ledger bytes must match, and its named replacement commits must reach the tag and account for the dropped files. Missing, unrelated or altered reviews keep the delivery held.
