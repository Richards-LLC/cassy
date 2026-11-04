### Fixed — CI

- **Keep worker recovery tests independent of runner disk space** — The
  shutdown recovery fixture disables disk admission only in its test config,
  with a regression run at five GiB available. Queue retries that stop before
  writing a report now retain and name the original failures in a FAIL receipt.
