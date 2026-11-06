### Fixed — Merge reconciliation

- **Candidate failures name the recovery step** — Git refusals during merge
  reconciliation now tell the supervisor to fetch origin, reconcile the source
  and target refs, and retry, alongside the command and its error details.
