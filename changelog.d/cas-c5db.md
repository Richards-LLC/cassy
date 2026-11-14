### Fixed — Merge reconciliation

- **Queue landings over newer target work reconcile** — when the merge queue
  lands an approved delivery on top of other work, reconciliation keeps the
  approval instead of holding the merge for a second review. A target change
  to the delivered files is still refused.
