### Fixed — Factory

- **Merge parked target updates without re-parking** — Merging an epic into a pushed parked delivery now advances its delivery anchor with an audit note, including after the worker starts its next task. Only target-reachable imports qualify; new task commits, unpublished updates and foreign merges still refuse. CI and review checks use the updated tip.
