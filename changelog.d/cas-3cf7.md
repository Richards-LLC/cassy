### Fixed — Factory

- **Park refuses a delivery missing its doc fragments** — Was: a product
  delivery without `docs/product-overview.d/<task>.md`, or a bug fix on a live
  journey's owned files without a bug family naming it, parked and then lost a
  verifier round to the miss. Now: park lists both misses at once (G-167
  overview fragment, G-168 bug family), with `no overview: <reason>` and
  `no bug family: <reason>` decision notes as the escapes; docs- and test-only
  deliveries are unaffected.
