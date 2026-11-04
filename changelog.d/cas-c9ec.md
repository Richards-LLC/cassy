### Fixed — Sessions

- **/finish leaves nothing running, and the session can end** — /finish now stops the background shells, monitors, workflows, servers and reminders the session started and lists them, so Claude Code's exit no longer stops to ask about leftover background tasks. A session whose handoff names each task it still holds may now end with those tasks left open for the next session, instead of being told to close them first.
