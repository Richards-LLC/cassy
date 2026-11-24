### Fixed — Pre-merge verification

- **A stalled verifier no longer pins its dispatch** — Was: when a verifier's
  thread stopped inside a host process that was still running, the expired
  dispatch stayed Claimed and every merge retry was refused with "finish this
  exact verdict". Now: past the deadline, an owning supervisor's retry releases
  a Claimed dispatch whose verifier has neither a fresh heartbeat nor an active
  verifier lane, and starts a replacement on the same dispatch.
