### Fixed — Factory

- **Inspectable Claude boot refusals** — Missing SessionStart canaries retain bounded, redacted startup output before the pane disappears. Debug and liveness inspection can find failed boots by worker or spawn request without a registered agent; task bindings are released only after completed teardown.
