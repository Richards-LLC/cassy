### Fixed — Commander

- **History retries survive late replies** — A delayed newest page preserves the error and retry cursor for an unanswered older page. Each history read keeps its own deadline and completion state.
