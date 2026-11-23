### Fixed — CI watcher

- **Cassy no longer trips GitHub's secondary rate limit every half hour** — Was: every minute the CI watcher ran one `gh pr list` per parked delivery, about 111 a minute across this machine's projects, most of them train lanes that never get a PR, until GitHub refused all Cassy calls for ~30 minutes at a time. Now: a branch's PR lookup is shared machine-wide and re-read only while a PR is open or the open-PR listing the poll already reads names the branch, otherwise every 10 minutes (about 11 calls a minute).
