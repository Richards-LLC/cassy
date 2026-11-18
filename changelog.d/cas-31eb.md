### Fixed — Test evidence

- **Uncertain test selection runs broader proof** — Unclassified tests now widen to one capped pass of the test file, JavaScript package or Rust crate. Park consumes a separate receipt; identified changed tests still require five passes. Deleting an unrelated inline test no longer demands annotations on unchanged shared callbacks, and multiline TypeScript import types preserve test locations. Dedicated Jest configs and caller name filters remain executable through the capped runner.
