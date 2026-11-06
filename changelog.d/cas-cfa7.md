### Fixed — Factory

- **Keep capped test temporary files on worker scratch** — Worker and verifier
  Cargo runs now place temporary files and fixture HOMEs in their private
  target's `tmp/` directory, which existing target GC reclaims. Large binary
  fixtures no longer inherit a memory-backed system temporary directory.
