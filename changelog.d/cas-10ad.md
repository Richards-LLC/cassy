### Fixed — Managed file sync

- **Clear managed staging after sync** — `cas update --sync` creates one versioned chore commit for purely managed changes on a named branch, including the main checkout, and recovers staged changes from earlier syncs. Project edits stay staged. Merge refusals identify purely managed sync residue and name the clearing command.
