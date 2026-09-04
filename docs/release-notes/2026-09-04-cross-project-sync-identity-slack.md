# Slack draft — Cassy v3.15.8 runtime release

Channel: #cas-internal (C0B44GUKDK2)

Status: POSTED 2026-09-04T16:47Z via MechaCassy (channel `cas-internal`).

User top-level

```text
*Live on production — User — Cassy v3.15.8*
Was: Cassy could mix up which project a task belonged to. Now: every project keeps only its own work, and mixed data is found and repaired.
```

User reply

```text
• *Project updates* — Was: refreshing several projects could put one project's tasks in another project's list. Now: each project keeps its own tasks and team updates.

• *Team updates* — Was: bringing in team updates could add another project's tasks or claim tasks with no clear owner. Now: uncertain tasks wait safely until their owner is clear.

• *Cleanup* — Was: routine background refreshes could stop cleanup, and old records could be skipped. Now: cleanup finishes in a fresh session and the next check looks at everything again.

• *Health check* — Was: health checks did not clearly show when project data was mixed or how to fix duplicate tasks. Now: the check names the problem and gives a clear repair command.

*Install*
`cas update`, or download the Linux x86_64 or macOS ARM64 archive from the GitHub release. SHA-256: `93647426687ff766e1cabfda7c15bf00ae4e1d678560c0bc84c5183966c9a470` · `4eda164584e57cfbf45394dcf0e1576d6a940e1388721ae6563347308fcd4819`.
```

Dev top-level

```text
*Live on production — Dev — Cassy v3.15.8*
Was: multi-root sync reused one identity. Now: every root carries its own cloud scope through update, pull, push, purge, and registration.
```

Dev reply

```text
• *Root identity* — Was: root-aware paths could use one cached identity from the current working directory. Now: `cas_root` flows through update, `CloudSyncer`, team, knowledge, registration, and MCP sync paths.

• *Ownership* — Was: team-task ingest wrote rows before proving origin and filled null origins from request scope. Now: foreign or identity-free rows are parked. Null origins require server attestation.

• *Purge state* — Was: session-start access refreshes counted as content and watermarks survived deletion. Now: the guard covers task, rule, and skill writes. Purge clears every `last_team_pull_at_` key before re-pull.

• *Doctor evidence* — Was: diagnostics lacked root-scoped metadata and collision commands. Now: `cloud identity metadata` compares watermarks, registrations, and knowledge-push identity to the root and prints remediation.

*Validation*
Release gate and full test suite green on the published tree. Archives: `cas-x86_64-unknown-linux-gnu.tar.gz` (`93647426687ff766e1cabfda7c15bf00ae4e1d678560c0bc84c5183966c9a470`) and `cas-aarch64-apple-darwin.tar.gz` (`4eda164584e57cfbf45394dcf0e1576d6a940e1388721ae6563347308fcd4819`).
```

## POSTED

- **Posted at (UTC):** `2026-09-04T16:47Z`
- **Channel:** `#cas-internal` (`C0B44GUKDK2`)
- **User top-level:** `message_id=1788540417.203839` · https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788540417203839
- **User reply:** `message_id=1788540423.561269` · https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788540423561269?thread_ts=1788540417.203839&cid=C0B44GUKDK2
- **Dev top-level:** `message_id=1788540427.142239` · https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788540427142239
- **Dev reply:** `message_id=1788540434.179849` · https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788540434179849?thread_ts=1788540427.142239&cid=C0B44GUKDK2
