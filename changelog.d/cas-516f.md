### Fixed — Factory

- **A supervisor's startup message no longer overflows the harness inline cap** — Was: a predecessor with a large lane (100+ pending rows, 25 integrated deliveries) pushed the supervisor SessionStart to 10–15 KB, so the harness showed a 2 KB preview. Now: the full integrated-delivery list is also written to a recovery receipt file named in the message, and an over-budget start collapses the handoff and that list to counts plus receipt paths.
