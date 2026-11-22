### Fixed

- Lane test-impact selection now attributes a changed integration case file to the suite that compiles it through `#[path]` or `mod`, so an edit under a case directory such as `distributed_factory_test_cases/` runs that suite first instead of widening to every integration suite.
