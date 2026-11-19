### Fixed — CI

- **Portable step watchdog diagnostics** — Find build processes through the system process table on Linux and macOS. Failed process telemetry no longer prevents deadline cleanup or the one allowed retry, and the watchdog self-test runs on both platforms.
