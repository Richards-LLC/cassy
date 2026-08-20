---
name: cas-wizard
description: Generate an interactive Bash wizard for human-only setup, secrets, dashboard, cutover, or migration steps. Do not use it for work the agent can perform directly.
managed_by: cas
---

# Human Procedure Wizard

Imported and adapted from mattpocock/skills `wizard`, MIT © 2026 Matt Pocock.

Use a stage-by-stage Bash wizard for a repeatable manual procedure. Inspect the project, enumerate every manual step, URL, destination, and secret classification, then confirm the ordered plan with the user. Copy `template.sh`; edit only after `STAGES`; keep one focused action per stage; hide secrets and confirm irreversible actions.

Run `bash -n` and static tracing, never an end-to-end interactive run. Keep decisions and proof in `mcp__cas__task`; do not create parallel tracker, setup, or context files.
