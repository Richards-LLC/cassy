### Fixed — CI

- **Lane CI pins the agent-doc budget check** — Was: nothing pinned that a lane editing a builtin agent doc runs the agent-definition contract test, so a byte-budget breach surfaced first in the merge queue. Now: a fixture proves the lane selector runs it for a builtin agent doc edit and not for an unrelated edit.
