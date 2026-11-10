### Fixed — Factory

- **Quiet child work stays active** — Worker status and stall recovery recognize measured CPU or output writes from owned child processes, including non-Rust test logs. The factory progress watchdog counts that advancement, while silent or exited children still allow recovery and escalation.
