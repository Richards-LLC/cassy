### Fixed — Release validation

- **Run the source suite once per release** — Plumbing-only prepared train pushes and merge-queue entries adopt the train's verified full-set receipt, including compiled documentation and release reports. A durable adoption record binds the release head to the original source proof for retries. Source changes retain the full suite; release compilation, launch, asset and packaging checks still run.
