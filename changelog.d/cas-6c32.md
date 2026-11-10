### Fixed — Factory coordination

- **Requested worker replies wake the supervisor** — The first settling answer to a supervisor's explicit ask bypasses the hourly wake cap and burst debounce even when `kind` is omitted or set to `status`. Replies to instructions, repeated answers and unsolicited status remain capped; explicit quiet intent stays quiet. Fixes GH #1780 part 1.
