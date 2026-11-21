### Fixed — Factory

- **Local-merge deliveries merge without a pushed branch** — Was: an approved
  `local_merge` delivery whose worker checkout had moved on, merged through
  recovery or from a reused lane, still fetched and required the worker's
  branch on origin. With origin unavailable, the merge was refused as a dirty
  checkout even though the delivered commit was clean. Now: it merges its exact
  local commit with no origin check. Pushed-branch deliveries still require the
  matching pushed ref.
