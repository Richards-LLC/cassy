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
| CSS or source-text assertion | `expect(` on a variable read from a CSS, `main.ts` or HTML file; exclude DOM queries and documented accessibility-property contracts; inspect whether the assertion proves an effective outcome | 8/10 with the DOM exclusion, 5/16 without |
| real timers | `setTimeout`, `sleep`, a `waitFor` with a large timeout and no fake-timer setup | measure by file time |
| dynamic import in a test body | `await import(` inside `it(` or `test(` | check each: some are mock isolation |
| unbounded lazy regex | `.*?` or `[\s\S]*?` with no length bound in an assertion over a large string | check each |
| token-name contract | assertions that a CSS variable name exists | candidate implementation pin; distinguish a named accessibility contract |
| subset claimed as full suite | a delivery note says "full suite" but the command had a path or `-t` filter, or the count differs from the runner's total | per-delivery check candidate |

## Any language

- **Duplicate:** map assertion to assertion within the same module, then across layers (integration tests against unit tests, end-to-end tests against component tests). Quote both sides. A file imported by another test is not deleted.
- **Self-contained test:** a test file that imports nothing from the project and checks its own data (for example raw SQL on its own schema).
- **Flaky:** a test that failed then passed on retry in the CI retry data.

## Screenshot and e2e

Flag an e2e test that takes screenshots or dumps the DOM and has no `expect` of its own (reference audit: 1/3). End-to-end tests that finish before the critical path ends do not shorten the run; say so in the ledger.

## Browser smells and silent green

Treat every match below as a review candidate; sample 10 and retain the precision floor from the skill. A matcher name alone is not a defect.

| Candidate detector | Confirmed defect | Replacement / evidence |
| --- | --- | --- |
| CSS/Quasar class locators | Styling structure is the primary identity | Inspect role/label/test id; retain intentional machine selector contracts |
| Banned strictness fix: .first() | It suppresses strictness rather than testing specified order | Narrow a semantic container and prove the promised outcome |
| Banned readiness substitute: networkidle | It substitutes connection silence for user readiness | Web-first state assertion or the required response plus UI outcome |
| Banned completion substitute: waitForTimeout | A sleep substitutes for completion | Event/barrier; controlled clock only for time logic |
| .catch(() => undefined) | It swallows a required action/assertion failure | Propagate the error; preserve intentional context-close cleanup separately |

The five confirmed patterns above are banned authoring fixes. Also inspect isVisible() branching, soft returns, copy/word-list assertions and retries used to mask failures. toHaveText on fixture data and accessible-name locator inputs remain valid.

Trace each required test command to an enabled CI job and uploaded full report. Flag missing-secret conditions that skip every test step, zero globally executed required tests, unexplained skips and disabled required lanes; an alternate real lane can satisfy coverage. Empty individual shards and declared optional lanes are not silent green.

For a measured flake hunt, use a named selection with --repeat-each and --shuffle <seed>, plus the failures console reporter. Keep full structured reports and original failure evidence. Report failures/executions and seed/worker count; never infer a cause from a single rerun.

Faith review pairs each title with its final assertion and independent expected value. Record title, assertion location, meaningful red control and the exact missing promise. Read-only audits report gaps; they never weaken assertions or auto-run the healer. A separately authorized scratch diagnosis may accept reviewed locator/wait fixes only, never assertion changes.
