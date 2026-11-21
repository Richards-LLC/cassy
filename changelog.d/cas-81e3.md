### Fixed — Commander journeys

- **Playwright 1.65.1 and role-based HUB-J27 controls** — the Commander
  journey suite now runs on `@playwright/test` 1.65.1; CI installs the
  matching Chromium from the lockfile. The supervisor-commands journey finds
  its composer, Send button, command palette and command buttons by role and
  accessible name instead of IDs and data attributes.
