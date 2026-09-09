# Slack draft — `cas update` keeps the post-swap refresh receipt (main merge, PR #788)

Channel: #cas-internal. Deploy target: Live on production (main). Reaches installed hosts with the next runtime release.

## User thread

Top-level:
Live on production — User — Was: after an update, Cassy could say the refresh outcome was unknown even though it had just printed which projects failed. Now: it keeps that receipt and tells you exactly which projects need attention and what to run.

Reply:
• Refresh receipt kept — Was: `cas update` swapped the binary, then reported "post-swap child exited without a refresh receipt; the refresh outcome is unknown" whenever any project's refresh failed, even though the per-project summary had scrolled past. Now: the refresh writes its receipt to a file the updater reads back, so the final message names the projects that did not refresh and the command to rerun.
• Same detail in `--json` — Was: a JSON update run could lose the receipt when the child's output carried none. Now: the file receipt fills the gap and carries a `refresh_status` field.

## Dev thread

Top-level:
Live on production — Dev — Was: the plain post-swap refresh child inherited stdio, so the parent had no receipt transport and mapped every non-zero exit to `refresh_failed_no_receipt`. Now: a hidden `--refresh-receipt <file>` side-channel carries the child's `project_refresh_receipt_json` (with `refresh_status`) back to the parent on both plain and JSON paths (PR #788).

Reply:
• Receipt transport — Was: `run_post_swap_refresh` used `command.status()` with inherited stdio for non-JSON updates, so the child's structured receipt never reached the parent. Now: `build_post_swap_command_with_receipt` passes `--refresh-receipt <tempfile>`; `refresh_all_projects` writes `project_refresh_receipt_json` there (new `refresh_status`); the parent reads it via `read_refresh_receipt` and renders `post_swap_refresh_failed_hint` with the failing projects.
• JSON path — Was: only the last JSON document on stdout counted. Now: `parse_refresh_receipt(stdout).or_else(read_refresh_receipt(file))`.
• Compatibility — children that predate the flag are still handled by the version probe; the parent side takes effect from the next update run by a binary that contains it.

Tests: plain partial-receipt transport and guidance, receipt serialization to disk; JSON, spawn-failure, stale-version, and no-receipt paths unchanged (update module 95/95). Real built-binary run and terminal-qa PASS 12 runs. PR #788.

## POSTED
Posted 2026-09-09 17:33Z via the MechaCassy hub to #cas-internal (C0B44GUKDK2):
- User top-level ts 1788975207.978089 https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788975207978089 (reply ts 1788975214.339129)
- Dev top-level ts 1788975216.567679 https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788975216567679 (reply ts 1788975230.687449)
