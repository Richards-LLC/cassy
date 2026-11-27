### Changed — Factory

- **One task per worker** — Workers retire when their task is parked, closed or cancelled. Further tasks require a fresh worker. Claude and Codex workers reaching 200,000 latest-call input tokens receive a checkpoint request and recycle on their current task after saving a clean checkpoint.
