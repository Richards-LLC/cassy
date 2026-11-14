### Changed — CI

- **Bubblewrap installs only where sandbox tests run** — Build-only Linux jobs no longer install bubblewrap through the shared Rust setup, and jobs that run verifier sandbox tests install it exactly once.
