### Fixed — Task QA context

- **Read metadata from the assigned checkout** — Task prediction and finding main-journey validation now use the task's registered worker checkout after checking its Git repository against the work target. Task show presents fresh QA impact separately from saved history, and start refreshes its impact context; a stale primary checkout no longer hides tracked worker registries or product context.
