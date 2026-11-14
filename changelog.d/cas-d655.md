### Fixed — Cloud sync

- **Guard team writes with known revisions** — Team pushes now carry observed server revisions and retain a rejected base until reconciliation, preventing retries from silently dropping back to timestamp writes. After an authenticated pull reconciles a row, its queued body and observed base advance together so the pending edit can reach Cloud. Unreconciled revision conflicts remain explicitly visible instead of exhausting retries. Atomic preservation of concurrent task-note appends depends on petra-stella-cloud#268.
- **Name missing artifact references** — Artifact lookup identifies a missing publication reference and conditionally directs recovery through receiver and publisher sync without claiming Cloud was checked.
