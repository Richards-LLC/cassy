# Slack draft — Cassy v3.15.8 runtime release

Channel: #cas-internal (C0B44GUKDK2)

**Status:** Draft. Do not post until the v3.15.8 release is published.

## User thread

**Top-level:**
Live on production — **User** — Cassy v3.15.8: projects used to sync under whichever directory launched the refresh; now every project stays in its own cloud space and foreign data is kept out of local work.

**Reply (Was → Now):**
- Was: refreshing several projects with `cas update` could make them all use the launch directory's cloud scope, so another project's tasks or team data could appear locally. Now: each project syncs under its own cloud identity, keeping project data in the right place.
- Was: a team pull could write another project's tasks into the local project, or treat a task with no clear owner as local. Now: only tasks with trustworthy ownership are written, while foreign or ownerless tasks wait safely instead of being claimed.
- Was: `cas cloud purge-foreign` could be blocked by routine session refreshes, and an old team-pull watermark could let the next pull skip rows that needed another look. Now: routine refreshes no longer block the purge, and the purge clears team-pull watermarks so the next pull checks a complete snapshot again.
- Was: `cas doctor` could not clearly identify cloud identity metadata belonging to another project, and collision rows lacked a clear safe next step. Now: the check names foreign watermarks and registrations with the exact command to fix them, and collision rows show a title-confirmed operator path.

## Dev thread

**Top-level:**
Live on production — **Dev** — Cassy v3.15.8: cloud identity is explicit per `cas_root`, team-pull ownership is server-attested, purge resets scoped watermarks, and only remote-shaped pin collisions fail closed.

**Reply (Was → Now):**
- Was: root-aware update and cloud paths could fall through to a global cached identity derived from the current working directory, so every project in a refresh shared one wire scope. Now: `cas_root` is threaded through update, `CloudSyncer`, team push/pull, registration, knowledge sync, and related MCP paths; each request uses that root's resolved identity.
- Was: team-task ingest upserted rows without first proving that their origin belonged to the requested project, and null origins were stamped with the requesting scope. Now: explicit foreign or identity-free rows are parked, while a null origin can be accepted only from the server-attested project identity.
- Was: purge safety counted observational session-start entry refreshes as pending content, and retained team-pull watermarks after foreign-row deletion. Now: the pending-content guard covers task, rule, and skill writes, and purge deletes every `last_team_pull_at_` key before its project-scoped re-pull.
- Was: doctor surfaced foreign local rows without a root-scoped cloud metadata check or a complete remediation path, and retained collision reports did not expose the safe operator command. Now: `cloud identity metadata` checks watermark, registration, and knowledge-push identity values against the current root and prints the remediation command; retained collisions include the title-confirmation path.
