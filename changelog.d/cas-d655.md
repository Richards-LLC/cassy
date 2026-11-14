### Fixed — Cloud sync

- **Guard team writes with known revisions** — Team pushes now carry observed server revisions and retain a rejected base until reconciliation, preventing retries from silently dropping back to timestamp writes. Accepted revisions are recorded for the next push. Atomic preservation of concurrent task-note appends depends on petra-stella-cloud#268.
- **Name missing artifact references** — Artifact lookup identifies a missing publication reference and explains why repeating receiver sync cannot recover a pointer absent from Cloud.
