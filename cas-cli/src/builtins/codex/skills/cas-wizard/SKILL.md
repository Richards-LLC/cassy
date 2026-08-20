---
name: cas-wizard
description: Generate an interactive Bash wizard for human-only setup, secrets, dashboard, cutover, or migration steps. Do not use it for work the agent can perform directly.
managed_by: cas
---

# Human Procedure Wizard

Imported and adapted from mattpocock/skills `wizard`, MIT © 2026 Matt Pocock.

Use a wizard for a repeatable manual procedure: a person drives external dashboards and confirms irreversible steps while the script gives one precise stage at a time. First inspect the project and enumerate every manual step, its source URL, destination, and whether its value is secret. Confirm the ordered stage plan with the user before authoring it.

Copy [template.sh](template.sh) to a task-appropriate script path and edit only the section after `STAGES`. Give each stage one focused action, open the exact URL before asking for a value, hide secrets, and confirm before irreversible actions. Keep generated procedures in the task’s approved output area unless the user asks for a maintained repository runbook.

Run `bash -n` and static tracing only; do not run a wizard end-to-end because it opens browsers and blocks on human input. Keep task decisions and proof in `mcp__cs__task`; this skill never creates a parallel tracker, setup system, or context file.
