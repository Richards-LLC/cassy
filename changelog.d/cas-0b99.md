### Fixed — Worker test runner

- **Auto-discovered test targets accepted** — Was: the capped runner refused `--test` for Cargo auto-discovered integration tests such as cas-factory spec_resolver, so workers could not get a receipt. Now: tests/<name>.rs and tests/<name>/main.rs are accepted unless the package sets autotests = false.
