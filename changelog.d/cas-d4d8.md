### Fixed — Factory

- **Keep integrated history out of a reset task's delivery** — Restarting an open task after its worker branch resets to the integration target records that tip as the start base. Verification also uses the newer origin target when the local target lags, so an approved no-delivery close does not reject already integrated history as task work.
