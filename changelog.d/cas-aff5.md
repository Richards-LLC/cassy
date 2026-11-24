### Fixed — Verification

- **An epic's mention of another task's artifact no longer blocks its children's merges** — Was: any `art-` ID in a parent epic's text was treated as the epic's own artifact, so mentioning another task's artifact refused every child merge with "artifact belongs to another task". Now: each cited ID resolves against the task that owns it within the project. A mention that no task in scope owns reaches the verifier as missing context instead of refusing the merge. A resolved artifact must still be available and unrevoked, and its bytes still verify.
