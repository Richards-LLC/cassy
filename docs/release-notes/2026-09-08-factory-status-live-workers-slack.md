# Slack draft — factory status shows live workers (main merge, PR #760)

Channel: #cas-internal. Deploy target: Live on production (main).

## User thread

Top-level:
Live on production · User · The factory status view now shows every worker that is actually running, not just the ones the daemon happened to record.

Reply:
Was → the status and agents views hid workers started through the coordination tools, so an operator could see one agent while three were working. Now → both views read the live worker registry, and a worker shows up the moment it registers.

## Dev thread

Top-level:
Live on production · Dev · `cas factory status` and `cas factory agents` filter agents through the registry-backed session roster instead of the daemon metadata roster.

Reply:
Was → `session_agent_name_set` in `cli/factory/queries.rs` read `metadata.workers`, which MCP `spawn_workers` never appends to, so live registry workers were dropped from status, agents, and activity filtering. Now → it extends with `SessionInfo::worker_names()` (registry with metadata fallback); regression test covers an empty roster with an active registry worker. PR #760.

## POSTED
Posted 2026-09-08 via claude.ai Slack MCP to #cas-internal (C0B44GUKDK2):
- User top-level ts 1788895911.486499 https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788895911486499
- User reply ts 1788895922.451619
- Dev top-level ts 1788895923.462889 https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788895923462889
- Dev reply: see thread of 1788895923.462889
Note: mecha-cassy MCP was unusable (malformed resultType), filed Richards-LLC/mecha-cassy#8.
