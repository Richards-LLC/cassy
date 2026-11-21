### Fixed — Factory

- **Evidence sync preserves newer published ledgers** — An older parked evidence
  manifest no longer replaces a local ledger whose published revision is newer.
  Sealed train parks refuse stale evidence manifests and identify the newer
  artifact for re-parking; newer manifests still restore older local evidence.
