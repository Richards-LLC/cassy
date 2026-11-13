### Fixed — Release trains

- **Retargeting to the next train moves the parent too** — Was: retargeting a
  parked task from a frozen train to the next train's branch left it under the
  frozen train, so its next start was refused until a separate epic update.
  Now: the same update moves the parent to the next train.
