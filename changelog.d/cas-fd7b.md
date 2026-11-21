### Fixed — Factory

- **Automatic premerge repair respects the class audit** — A paperwork-only
  premerge rejection no longer returns a task to its worker automatically when
  the task has recorded failing tests and no class audit. It holds for the
  supervisor with the CLASS_AUDIT_REQUIRED refusal, so the repair names the
  whole test class through request_changes.
