### Fixed — Factory

- **A refused reexec names what it is waiting for** — Was: on a busy session
  `cas factory reexec` refused with "Durable daemon jobs did not flush before
  reexec" and named no job, so a retry minutes later was a guess. Now: the
  refusal lists each unfinished daemon job by lane, kind, key and age, says
  when a write is still waiting for the store writer, counts any store writes
  still queued, and says the session keeps running unchanged until a retry.
