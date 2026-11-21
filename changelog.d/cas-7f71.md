### Fixed — Task close

- **Commitless spikes retain their approved no-delivery proof** — A worker's
  `no_delivery` close now parks for the train owner without recording a factory
  delivery branch. The owner can close the spike using its existing approval,
  without a proof-scope correction or another verification.
