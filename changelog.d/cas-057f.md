### Fixed — Release cuts

- **Preview closeout before enqueue** — Release cuts evaluate the candidate's per-member close decisions before pushing. A nonempty cut with no closable members refuses; cut receipts name every missing review and record candidate build time. `cas release preview --scope` replays a historical release without changing its store.
