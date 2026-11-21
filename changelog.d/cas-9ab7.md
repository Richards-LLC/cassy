### Fixed — Factory

- **Back-to-back Codex spawns pick the same account** — When several spawns ran at once, their `codex login status` checks could time out. A spawn that couldn't confirm the login skipped the registered account and refused with "the default Codex home is missing", one call after the same spawn had succeeded. A timed-out login check now still selects that registered Codex account. Logged-out, disabled, stale or never-used accounts are still skipped, and the spawn's own login check still verifies the chosen account before launch.
