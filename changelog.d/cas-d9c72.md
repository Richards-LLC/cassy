### Fixed — Close gate

- **Five-pass proof survives the close gate's own doc fragments** — Was: adding
  the changelog, product-overview or bug-family fragment that park asks for
  voided an earlier five-pass Rust receipt, because any `.yaml` counted as a
  test input. Now: those fragments carry the proof unless a Rust source names
  them. A stability refusal also names the earlier proof and the changed inputs
  (or broken ancestry) that stopped it carrying.
