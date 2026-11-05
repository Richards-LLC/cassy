### Changed — Session start

- **Memories are fetched on demand** — Session start no longer pre-ranks memories. It used to list "Helpful Memories" and "Related to Current Work", but those lists were cut for space in every measured worker session and were never opened. Session start now shows one line pointing to `search action=context`, which returns the full ranked list when asked. Set `hooks.session_start_memories = true` to list them at start again. By default the friction ledger records no session-context memories, and the current handoff no longer counts as one.
