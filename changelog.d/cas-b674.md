### Added — Factory

- **One train owner across machines** — Start a peer factory with `--worker-host <train>` to run workers under the train owner's session without a local supervisor. Workers send status, blockers and parked deliveries directly to that owner; the owner alone verifies, merges, returns work, waives QA and closes deliveries. Portable artifact and test-receipt claims are checked before remote admission, and obsolete peer-supervisor approval and merge-intent relays no longer grant authority.
