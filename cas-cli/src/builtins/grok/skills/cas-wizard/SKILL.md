---
name: cas-wizard
description: Generate an interactive Bash wizard for human-only setup, secrets, dashboard, cutover, or migration steps. Do not use it for work the agent can perform directly.
managed_by: cas
---

# Human Procedure Wizard

Imported and adapted from mattpocock/skills `wizard`, MIT © 2026 Matt Pocock.

Use a stage-by-stage Bash wizard for repeatable manual work. Confirm the step/URL/destination/secret plan, copy `template.sh`, and edit only after `STAGES`. Keep one action per stage, hide secrets, and confirm irreversible actions. Run `bash -n`, not an end-to-end interactive wizard; retain decisions in `mcp__cas__task` without parallel setup or tracker files.
