### Fixed — CLI

- **`cas factory worker-check --cas-root` no longer claims CAS_ROOT wins** — Was: with CAS_ROOT naming another store, the capped runner printed the "CAS_ROOT supplies the default store" notice although it locks and writes its receipts in the store named by `--cas-root`. Now: the runner names its store explicitly, so the notice stays silent for it; other commands without a store choice still print it.
