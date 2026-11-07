### Fixed — Factory

- **The automatic premerge verifier starts on an eligible account** — Was: on a host whose default `~/.codex` is logged out, the daemon-launched verifier used that home and stopped with "codex is not logged in", so the merge waited for a hand launch. Now: it chooses the account `spawn_workers` would (the recommended account, else a logged-in Codex home whose usage is not known yet).
