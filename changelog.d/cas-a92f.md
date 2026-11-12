### Fixed — Factory
- **Fresh worker capacity for task placement** — Peer routing now checks live workers against the machine cap (eight by default), reports the observation and its age, and uses a recovered local slot before retrying an unsent ask. Repeated spawns retain accepted or in-flight deliveries without sending a second route for the task.
