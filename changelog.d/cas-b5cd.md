### Added — Factory

- **Every factory check has a stable code** — Was: refusals were free-text
  headings with no way to count them per check. Now: each gate, guard,
  watchdog, notice and CI check declares a code at its source, `cas gates list`
  prints the registry measured from code, refusals end with their `[CODE]`, and
  `cas gates report --since <tag>` counts refusals, overrides and
  `cas gates false-positive` declarations per code.

- **Count script and merge decisions** — Script failures emit their registered
  codes; merge CI refusals and accepted overrides, including pre-push fast-row
  skips, record typed per-code facts.
