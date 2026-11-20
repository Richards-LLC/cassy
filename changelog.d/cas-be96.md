### Fixed — Factory

- **Delivery CI watches no longer poll GitHub forever** — Was: every parked
  delivery armed a CI-completion watch for a workflow named "CI", even in
  projects without one or for commits already on their target, so each watch
  polled GitHub every 30 seconds and drained the shared API budget. Now: the
  watch is armed only when the project defines that workflow and the commit is
  not yet on its target, and existing watches for a workflow the project lacks
  are not probed.
- **Bug filing under rate limit names the backoff** — Was: a filing held by
  the GitHub rate-limit backoff was reported as a missing or rejected
  credential. Now: it says the rate limit held the request.
