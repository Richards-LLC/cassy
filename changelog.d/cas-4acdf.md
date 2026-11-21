### Fixed — Factory

- **The long-command guard covers the supervisor and skips its verifiers** — Was: the new guard for long foreground commands skipped any supervisor whose hooks carry its own team identity, which every Agent Teams supervisor does, and refused task verifiers' Cargo runs before they reached the capped verifier lane. Now: it applies to the supervisor's own pane, and verifiers and other subagents keep their usual routing.
