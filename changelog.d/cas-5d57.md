### Fixed — Violet

- **CLI reply and reaction parity** — Managed replies accept explicit intent IDs and claim generations; `cas violet react` adds or removes a reaction through Violet.
- **Reply refusal diagnostics** — A reconciled `not_posted` result retains the provider's error and reason and the original uncertain response, without resending the write.

### Added — Reports

- **Client findings reports** — The shared `cas-client-report` skill prepares client findings as HTML and PDF, with accounting validation and routing to specialized delivery skills.

### Fixed — macOS

- **Stable opt-in update identity** — Updates use an existing local code-signing identity before relaunching, keeping the same designated requirement across versions. Hosts without that identity retain their existing signing behavior.
