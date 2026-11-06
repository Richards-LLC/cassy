### Fixed — Delivery verification

- **Conflicting merges retire stale approvals** — When a changed target prevents
  a reviewed merge candidate from being rebuilt, the prior approval is
  invalidated before the merge error is returned.
