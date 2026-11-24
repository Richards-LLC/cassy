### Fixed — Factory

- **Test-proof gate never demands a run that cannot happen** — Was: a worker
  could be refused at park with no way out. A Vitest spec that
  also imported Playwright's browser library was prescribed a Playwright run.
  A page no test imports was prescribed `vitest related`, which ran zero tests
  and recorded nothing; a named changed-test run that matched no runtime test
  failed the same way, and a package with no JavaScript change got a
  placeholder command. Deleted specs were named as package inputs. Journey
  specs the QA registry declares (deployed staging) owed five capped passes.
  A worker-check pass recorded in one worktree never counted for a close read
  from another checkout. Now: the test API import picks the runner. Any
  JavaScript selection that runs no test runs the owning package instead, and
  park accepts that package pass for the changed tests and inputs inside it.
  `worker-check -- <runner> <package>/package.json` runs a package directly.
  A named Rust selection that runs zero tests runs its crate once instead, and
  park accepts that crate pass for the changed tests; the Rust prescription
  names the crate, harness and exact test. Deleted files owe nothing.
  Registry journey specs whose Playwright config starts no local server (a
  deployed host), and helpers only those journeys import, follow the journey
  lane; locally served journeys stay owed by the worker. A pass at the exact commit counts from any worktree of the repository.
