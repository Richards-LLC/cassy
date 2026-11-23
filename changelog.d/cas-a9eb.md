### Fixed — CI watcher

- **Cassy no longer trips GitHub's secondary rate limit every half hour** — Was: every minute the CI watcher ran one `gh pr list` per parked delivery, about 111 a minute across this machine's projects, most of them train lanes that never get a PR, until GitHub refused all Cassy calls for ~30 minutes at a time. Now: one batched PR read per repository per poll covers every parked delivery and is matched locally, so 111 deliveries cost one call a poll instead of 111.
