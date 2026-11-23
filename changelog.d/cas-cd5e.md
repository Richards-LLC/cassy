### Fixed — Factory close proof

- **A close that will be refused says so in seconds** — Was: on a delivery
  that changed many Commander journey specs, park took 21 minutes to refuse
  for missing test receipts (GH #2331). Building the refusal's run commands
  asked each Playwright config, one process per file and per config, which
  configs owned every changed file, again for each widened scope, and re-read
  the whole package's imports each time; a config that cannot load on this
  host was asked again for every file. Now: within one park each runner config
  is listed once and each package's imports are read once, and a config that
  cannot load lists nothing. The same delivery is refused in 10 seconds
  instead of 464, with the same refusal.
