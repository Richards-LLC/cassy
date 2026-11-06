### Fixed — Factory

- **Clock time on agent messages** — Wakes, reminders, inbox digests and peer messages now carry the current host-local time in twelve-hour form with a zone. Supervisors must read the clock or use a measured start before stating a time or ETA.
