### Fixed — Factory guard

- **Workers can list Playwright tests** — Was: the browser test tier refused
  `playwright test --list` as an unfiltered suite run, though it only collects
  tests. Now: a collection-only `--list` is allowed; a run, `--full`, or an
  option that could consume `--list` as its value is still refused.
