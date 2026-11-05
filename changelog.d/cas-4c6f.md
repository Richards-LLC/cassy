### Fixed — tool status text names the evidence it actually observed

- **Status words follow their evidence** — Committed writes with unfinished follow-up no longer say the action is done.
Inbox suppression names obsolete instructions and current task states;
tracked-only assignment checks and stored worktree merge/removal records name
their scope. Merge publication verifies an equal cached ref against live origin
and reports unavailable publication as unconfirmed. Release reports count listed
changes, and doctor names local release-tag containment. Hook cleanup JSON uses
`no_candidates_found`/`processed` and `candidates` instead of `clean`/`done` and
the incorrect zero `files_checked`. See `docs/status-words.md` for definitions,
source checks, compatibility notes and named regressions.
