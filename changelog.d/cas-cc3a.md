### Fixed — Release trains

- **Retargeting work to the next train no longer strands it without a train** — Was:
  the new target was saved first, then the old train link was removed and the
  new one added as two more separate writes. A failure between them left the
  task with no parent train and with a target that a repeat of the same retarget
  refused as unchanged. Now: the target and the parent move are saved together,
  so a failure leaves both as they were and the retarget can be repeated. Only
  the parent link to the old train is removed.
