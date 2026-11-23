### Fixed — Director and watchdog notices

- **Routine capacity waits no longer page the supervisor** — When a capped build or test is refused for builder slots, memory pressure or disk headroom, Cassy records a capacity wait with a scheduled retry. The director now treats that worker as waiting rather than idle, so each refused attempt no longer sends a "detached job exited … message or reassign" notice. Real check failures still notify as before, and long waits keep their own escalation (GH #2132).
