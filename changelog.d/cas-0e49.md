### Fixed — Peer coordination

- **Show wake-cap holds to the sender** — Held peer messages now return one notice with the earliest retry time. Substantive replies linked with `in_reply_to` bypass the cap and debounce so release answers can proceed.
