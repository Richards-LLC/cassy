### Fixed — Releases

- **Reuse shared delivery proofs during release preview** — Release previews
  reuse successful proofs for the same immutable merge edge and live line
  positions across deliveries, avoiding repeated Git blame and lexical work
  while preserving ownership and review verdicts.
