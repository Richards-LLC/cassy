### Fixed — Factory workers

- **Rebase a live checkpoint without treating it as parked** — H-40 checks authoritative task, completion and review state before accepting a checkpoint rewrite. Parked and reviewed deliveries retain their history protection; worker recovery guidance requires an explicit lease for an authorized checkpoint rebase.
