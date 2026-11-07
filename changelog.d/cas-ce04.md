### Fixed — Supervisor wakes

- **No second wake for a message already read** — Was: a pointer wake still opened a supervisor turn after a hook or inbox_poll had shown the message, so the turn only said "[already shown]". Now: the wake is skipped when that row was already shown; unread rows still wake.
