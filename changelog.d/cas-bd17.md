### Fixed — Release

- **No-code report deliveries can close at release again** — Was: once
  verdicts were bound to delivery cycles, an explicit no-code delivery (which
  never has a cycle) could not be approved by its verifier, so the release
  cut held it as missing review and refused with "zero would-close". Now: a
  delivery with no cycle is approved by the verdict for the commit its
  dispatch reviewed, as before; cycle-bound deliveries are unchanged. The
  cut refusal also lists the hold reasons and the members missing review.
