### Fixed — Factory close proof

- **Tests built only for another OS are proven on that OS** — Was: park asked
  for five local capped passes of a changed `#[cfg(target_os = "linux")]` test
  on a macOS worker (and of a macOS-only test on Linux), where the capped
  runner selects no tests, so the delivery could never park (GH #2320). Now:
  park reads the test's `target_os` gate, from the function, its modules or
  its file. Green lane CI at the delivered tip proves a test that builds on
  Linux, and a test for any other OS needs five passes recorded on that OS.
  The refusal says which, and never asks for a local run.
