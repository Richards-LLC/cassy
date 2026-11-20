### Fixed — Factory

- **Close approved peer reviews without a supervisor branch** — Was: an
  unchanged review source could be rejected after supervisor ownership changed.
  Now: an explicit no-delivery close by a registered supervisor can prove the
  original source is empty without creating a branch or changing its audit
  record. Reachable task commits still refuse close, with their SHAs and refs.
