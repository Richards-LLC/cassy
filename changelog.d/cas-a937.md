### Fixed — Capped worker tests

- **Resolve Rust tools for isolated tests** — Named worker tests find Cargo in launch PATH, `CARGO_HOME/bin` or `~/.cargo/bin` and stage `rustc` and `rustdoc` from the resolved Cargo's toolchain directory, while keeping that directory out of the isolated test PATH.
