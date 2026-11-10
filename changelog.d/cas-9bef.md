### Fixed — CI

- **Keep fresh reruns in the queue** — The queued-run watchdog ages each workflow attempt from its own start time, so rerunning an older workflow gets the normal scheduling window. First attempts still expire when genuinely stale; reruns with no current start timestamp remain queued until their age can be established.
