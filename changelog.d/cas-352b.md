### Fixed — Factory verification

- **Return a clear hold while a merge candidate builds** — Immutable candidate preparation runs once in the background with a 20-second response wait. Slow builds retain their exact cache and notify the supervisor when ready or failed; retries still require the candidate's exact review before publication.
