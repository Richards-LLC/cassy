### Fixed — Factory

- **A supervisor's startup message no longer overflows the harness inline cap** — Was: a predecessor with a large lane (100+ pending rows, 25 integrated deliveries) pushed the supervisor SessionStart to 10–15 KB, so the harness showed a 2 KB preview. Now: the handoff and the integrated-delivery list show a bounded slice, the full list is written to a recovery receipt file named in the message, and an over-budget start degrades both to counts plus receipt paths.
