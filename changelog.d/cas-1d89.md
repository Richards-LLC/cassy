### Fixed — Factory

- **Cancel detached checks completely** — Signalling the printed worker-check PID now reaches its capped producer and waits for Cargo descendants to stop, leases to release and completion to be recorded, so the next check can start.
