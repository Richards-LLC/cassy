### Fixed — Factory

- **Preserve parked history during conflict recovery** — Task-show and resume guidance now merge the integration target into the parked branch, then push and re-park for push_branch deliveries. Local-only deliveries keep their branch local and request the supervisor merge.
