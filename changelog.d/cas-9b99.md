### Changed — Commander

- **Epic progress lives in Tasks & progress, for this project only** — Was: a
  strip of epic tabs sat above the conversation, listed other projects' epics,
  showed blank "EPIC" names and "—/—" placeholders, and gave no open versus
  merged split. Now: the right-hand Tasks & progress pane (and the phone sheet)
  shows the release train, then Live epics for the selected session's project
  across both machines, each with "N open · N in progress · N merged", a
  segmented bar and its tasks in place. Untitled epics and placeholder counts
  never render, and nothing sits above the conversation.
- **Every epic reports its open / in progress / merged split** — Was: session
  status measured epic progress only within a two-second budget, so most epics
  had no counts. Now: every epic carries the factory TUI's split from the one
  task snapshot status already loads, and epics with work in progress are
  measured before idle history.
