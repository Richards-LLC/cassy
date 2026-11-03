### Fixed — Verification

- **Review before publishing factory deliveries** — Was: required verification
  ran after a merge published. → Now: the same verifier reviews an immutable
  candidate first; PASS publishes its exact tree and closes with that approval,
  while FAIL or an unresolved verdict holds only that delivery. Tier zero and
  batch tier one exemptions stay unchanged. An unconfirmed push retains the
  source and restores its own local merge without discarding operator edits.
