### Fixed — CI

- **Case-file edits run their own suite** — Lane test-impact selection attributes a changed integration case file to the suite that compiles it through `#[path]` or `mod`. An edit under a case directory such as `distributed_factory_test_cases/` now runs that suite first instead of widening to every integration suite.
