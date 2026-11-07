### Fixed — Tests

- **Batch receipt early-answer test no longer flakes under load** — Was: it bounded the answer at 5 seconds of wall time, which parallel runs exceeded. Now: it asserts the short batch grace answered instead of the caller grace, independent of host speed.
