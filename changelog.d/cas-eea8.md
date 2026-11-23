### Fixed — Factory coordination

- **Actionable worker handoffs wake promptly** — Ready messages and merge requests bypass the supervisor message wake cap and burst debounce, as blockers already do. Status traffic remains capped and quiet messages remain inbox-only.
