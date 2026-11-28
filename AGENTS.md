<!-- CAS:BEGIN - This section is managed by CAS. Do not edit manually. -->
## Cassy: tasks, memory and context

Track work and knowledge in Cassy; tasks and memories persist across sessions.
Cassy's MCP tools are `task`, `memory` and `search`, named with your harness's prefix: `mcp__cas__` in Claude Code, `mcp__cs__` in Codex, `cas__` in Grok, `cas_` in OpenCode. Call them directly.

- `task`: action=create, start, close, ready.
- `memory`: action=remember.
- `search`: action=search.

Slack transport: use the `violet` skill and the Violet proxy for every Slack read (`violet.violet_read`) and post (`violet.violet_post`). Never use the Claude Slack connector or Codex Slack app. Pre-tool hooks deny non-Violet Slack writes by default; only an explicit operator setting `slack.transport=any` disables that guard.

Bug routing (`issues.repo` for project bugs; `issues.components.{cassy,violet,cloud}` for components): a Cassy defect that blocks your task: `system action=report_cas_bug`, refusal first in `actual`, then continue. It records the report locally and reaches GitHub only when the same refusal recurs, so never file per refusal, apology or workaround; in Cassy's source repo, find or staff a fix task.
Deliverables: use `cas-sow-delivery-report` for SOW deliveries, `cas-feature-report` for feature reports, `cas-release-notes` + `cas-release-report` for internal releases, `cas-client-report` for client findings, and `cas-customer-release-notes` for customer product updates; follow DESIGN.md and use real before/after pairs.
Release notes: when a merge reaches `staging` or `main` and docs/release-notes/RUBRIC.md exists, use the `cas-release-notes` skill and follow docs/release-notes/RUBRIC.md.
<!-- CAS:END -->

# Cassy source repository

`CLAUDE.md` imports this canonical file. Rust 1.88+, edition 2024.

- Build/check/test work: read [CONTRIBUTING](cas-cli/docs/CONTRIBUTING.md#build-assembly-and-ci-policy). Workers use capped compile checks and named targeted tests. Green lane CI on the exact tip is test proof; missing local receipts warn at park. Pre-push runs no-build fast rows. The full suite runs once on the landing tree; the merge queue proves the final tree before `main`. For web diffs, workers run touched vitest files and tsc; lane CI runs selected journeys with no separate receipt. The hook denies worker/reviewer `--full` or unfiltered Playwright. Park reads evidence and runs no checks. Use `ASSEMBLY_PROOF` only when close accepts that fallback. Keep `panic = "unwind"`.
- Module/store/hook work: [ARCHITECTURE](cas-cli/docs/ARCHITECTURE.md); navigation: [.claude/CODEMAP.md](.claude/CODEMAP.md).
- CLI/MCP/migration/skill changes: [CONTRIBUTING](cas-cli/docs/CONTRIBUTING.md).
- Bug diagnosis: trace the real handler or data and cite the confirming line or reproduced behavior before fixing; use `cas-diagnosing-bugs` for the loop.
- CAS runtime, verifier, hook, factory or builtin-skill bugs belong here, including incidents in downstream projects. Create an in-repo fix task.
- Releases and harness diaries: publication to Slack is mandatory; read [RELEASE_SLACK_RUBRIC](docs/RELEASE_SLACK_RUBRIC.md) before preparing either.
