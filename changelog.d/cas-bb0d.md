### Fixed — Factory

- **Supervisor clears follow live context pressure** — Repeated cached calls no longer exhaust the auto-clear budget. The latest call is measured against the reported model window, with a configured per-call cap when the window is unavailable. Auto-clear handoffs retain active work and readable pending summaries instead of task histories and raw JSON. Fixes #1665.
