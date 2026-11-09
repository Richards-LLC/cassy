### Fixed — Factory

- **Detached builds settle before worker recycle** — Recycling stops the worker's owned capped runs and checks their inherited target leases before tearing down its process scope, preventing surviving guards or Cargo children from locking out the replacement. Lease refusals no longer identify the retry as its own holder.
