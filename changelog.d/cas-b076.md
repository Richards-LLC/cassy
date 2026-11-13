### Added — Test audit

- **One test value and flake audit** — The shipped audit now scans Rust and JS/Playwright tests for timing guesses, instant teardown checks, shared resources, order dependence and unseeded randomness alongside dead/duplicate, style/wording and slow candidates. The same report includes bounded thirty-day CI and issue history, with native evidence and explicit uncertainty instead of guessing flake rates.
