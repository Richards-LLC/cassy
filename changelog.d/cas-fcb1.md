### Fixed — Factory

- **Test-proof gate never demands a run that cannot happen** —
  Was: a worker could be refused at park with no way out. A Vitest spec that
  also imported Playwright's browser library was prescribed a Playwright run.
  A page no test imports was prescribed `vitest related`, which ran zero tests
  and recorded nothing. Deleted specs were named as package inputs. Journey
  specs the QA registry declares (deployed staging) owed five capped passes.
  A worker-check pass recorded in one worktree never counted for a close read
  from another checkout. Now: the test API import picks the runner. A focused
  run that selects no test runs the owning package instead, and park accepts
  that package pass for any input inside it. Deleted files owe nothing.
  Registry journey specs, and helpers only journeys import, follow the journey
  lane. A pass at the exact commit counts from any worktree of the repository.
