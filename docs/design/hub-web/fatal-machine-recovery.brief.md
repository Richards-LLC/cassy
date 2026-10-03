# Stopped machine connection

Reader: An operator whose connected machine has stopped because this browser cannot run the required connection API.
Decision: Update to a supported browser and reload, rather than wait for a retry that cannot fix it.
Hero form: Retain the last session frame with its existing connection banner, and one machine Attention entry with the same reason and recovery.
Distinctive move: Explain the missing browser feature in plain words, keep the actual compatible browser versions from the transport reason, and state reload as the next step.
Omitted: New layout, retry semantics, pairing and session-only outage vocabulary; existing Pebble surfaces and tokens stay in use.

The machine's fatal verdict outranks a stale attach retry. Tasks retain last-state age but cannot promise reconnect. Existing footer machine-state precedence is explicitly pinned; duplicate session transport notice is resolved under the machine notice. Full technical reason remains behind Details.
