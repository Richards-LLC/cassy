### Fixed — Release

- **A closed no-code member no longer blocks the release cut** — Was: when a
  release train listed a no-code task (notes or report) that had already
  closed, the cut preview held it as missing release membership, so a train
  whose members were all closed was refused as "zero would-close". Now: the
  closed no-code member counts as already closed and the cut proceeds.
