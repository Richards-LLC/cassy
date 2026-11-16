### Fixed — Release flow

- **Frozen train repairs reuse the full set** — A repair targeting an actively
  frozen train can merge after the lane's fast and factory/PR checks pass,
  without waiting for its duplicate Rust impact jobs. After publication, Cassy
  attempts to cancel the exact repair lane if only impact jobs remain. The
  frozen tip's full set supplies Rust proof; ordinary targets keep existing
  admission.
