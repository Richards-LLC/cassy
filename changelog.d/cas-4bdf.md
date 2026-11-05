### Fixed — Releases

- **Require the fixes named in release notes** — Release preparation now checks task fragments and task IDs in branch commit subjects once, refusing when the recorded delivery commit or matching source patch is missing and naming the task and commit. The check also covers fragments already promoted or dropped with a missing fix; regenerated bundles and Cargo.lock do not affect patch equivalence.
