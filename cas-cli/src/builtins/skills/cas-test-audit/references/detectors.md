# Detector recipes

Each recipe is a starting grep or tool, not a verdict. Hand-check 10 flagged rows and record `held / flagged`. Reference-audit precision is shown where measured; re-measure on the new project before trusting it.

## Contents

- Rust
- TypeScript and JavaScript
- Any language
- Screenshot and e2e

## Rust

| pattern | recipe | reference-audit precision |
|---|---|---|
| dead code (production never calls it) | capped `cargo check --lib` with `dead_code` warnings; then `git grep -w <item>` split into test and production hits | rustc "no production caller": 105/106. The grep split alone: 7/55 real dead-code tests (the rest were wrappers whose tests must stay) |
| superseded | `git log -S<symbol>` on the item; cite the commit that removed the last production caller or marked it "kept for compatibility" | per-row, cite the commit |
| restates implementation: logic copy | the test body contains the same `split`, `trim` or loop as the function and never calls it | 10/23 copied logic |
| const equals literal, `default_*()`, round trip | `assert_eq!(CONST, literal)`, `assert_eq!(default_x(), literal)`, serialize then deserialize | 12/19, 14/15, 25/35 |
| snapshot, enum table, source read | `insta`, tables of every variant, `include_str!` of a source file | 0/84, 2/73, 3/58: noisy |
| real sleep | `thread::sleep`, `tokio::time::sleep` in tests; exclude sleeps inside a poll loop with a deadline | 231 tests flagged, 135 sleeps were inside poll loops |
| per-test rebuilt setup | the same fixture built in every test of a module; look for a shared helper | find by timing: slowest 5% of tests |
| assertion-free | a test whose only checks are `is_ok()`, `is_err()` or none | high when the body has no other assertion |

## TypeScript and JavaScript

| pattern | recipe | precision |
|---|---|---|
| unreferenced module | `npx fallow dead-code` (compare `--production` against default); confirm with a same-file reference check | 5/7 on hub-web |
| CSS or source-text assertion | `expect(` on a variable read from a CSS, `main.ts` or HTML file; exclude `querySelector` and other DOM queries | 8/10 with the DOM exclusion, 5/16 without |
| real timers | `setTimeout`, `sleep`, a `waitFor` with a large timeout and no fake-timer setup | measure by file time |
| dynamic import in a test body | `await import(` inside `it(` or `test(` | check each: some are mock isolation |
| unbounded lazy regex | `.*?` or `[\s\S]*?` with no length bound in an assertion over a large string | check each |
| token-name contract | assertions that a CSS variable name exists | style pinning: same ruling |
| subset claimed as full suite | a delivery note says "full suite" but the command had a path or `-t` filter, or the count differs from the runner's total | per-delivery check candidate |

## Any language

- **Duplicate:** map assertion to assertion within the same module, then across layers (integration tests against unit tests, end-to-end tests against component tests). Quote both sides. A file imported by another test is not deleted.
- **Self-contained test:** a test file that imports nothing from the project and checks its own data (for example raw SQL on its own schema).
- **Flaky:** a test that failed then passed on retry in the CI retry data.

## Screenshot and e2e

Flag an e2e test that takes screenshots or dumps the DOM and has no `expect` of its own (reference audit: 1/3). End-to-end tests that finish before the critical path ends do not shorten the run; say so in the ledger.
