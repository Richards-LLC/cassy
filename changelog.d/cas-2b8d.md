### Fixed — Factory

- **Keep a queued release train frozen** — Release tracking marks the train cut before publication under the merge target lock. New tasks open or join the next train, and merges into the frozen lane wait for landing. A failed cut-marker write stops publication with a retry command; unavailable reminders remain advisory. Max-age notices now tell supervisors to retarget unfinished deliveries explicitly.
