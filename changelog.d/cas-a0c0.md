### Fixed — Factory close proof

- **Tests built only for another OS are proven on that OS** — Was: park asked
  for five local capped passes of a changed `#[cfg(target_os = "linux")]` test
  on a macOS worker (and of a macOS-only test on Linux), where the capped
  runner selects no tests, so the delivery could never park (GH #2320). Now:
  park reads the test's `target_os` gate, from the function, its modules or
  its file. Lane CI's Rust test job passing at the delivered tip proves a test
  that builds on Linux (a lint-only green does not), and a test for any other
  OS needs five passes recorded on that OS, which park now reads from the
  receipts synced on the task. The refusal says which, and never asks for a
  local run.
