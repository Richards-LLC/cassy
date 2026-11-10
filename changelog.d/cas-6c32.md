### Fixed — Factory coordination

- **Requested worker replies wake the supervisor** — A worker's linked answer to its supervisor bypasses the hourly wake cap and burst debounce even when `kind` is omitted or set to `status`. Unsolicited status remains capped, and explicit quiet intent stays quiet. Fixes GH #1780 part 1.
