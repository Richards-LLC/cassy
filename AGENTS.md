<!-- CAS:BEGIN - This section is managed by CAS. Do not edit manually. -->
## Cassy: tasks, memory and context

Track work and knowledge in Cassy; tasks and memories persist across sessions.
Cassy's MCP tools are `task`, `memory` and `search`, named with your harness's prefix: `mcp__cas__` in Claude Code, `mcp__cs__` in Codex, `cas__` in Grok, `cas_` in OpenCode. Call them directly.

- `task`: action=create, start, close, ready.
- `memory`: action=remember.
- `search`: action=search.

Slack transport: use the `violet` skill and the Violet proxy for every Slack read (`violet.violet_read`) and post (`violet.violet_post`). Never use the Claude Slack connector or Codex Slack app. Pre-tool hooks deny non-Violet Slack writes by default; only an explicit operator setting `slack.transport=any` disables that guard.

Bug routing (`issues.repo` for project bugs; `issues.components.{cassy,violet,cloud}` for components): Apologies, Cassy workarounds, relays Cassy missed, or untrue Cassy tool output mean Cassy defects. File immediately to issues.components.cassy with repro, evidence and suggested fix, then continue; in Cassy's source repo, create and staff the fix task immediately.
Deliverables: use `cas-sow-delivery-report` for SOW deliveries, `cas-feature-report` for feature reports, `cas-release-notes` + `cas-release-report` for internal releases, and `cas-customer-release-notes` for client updates; follow DESIGN.md and use real before/after pairs.
Release notes: when a merge reaches `staging` or `main` and docs/release-notes/RUBRIC.md exists, use the `cas-release-notes` skill and follow docs/release-notes/RUBRIC.md.
<!-- CAS:END -->

# Cassy source repository

`CLAUDE.md` imports this canonical file. Rust 1.88+, edition 2024.

- Build/check/test work: read [CONTRIBUTING](cas-cli/docs/CONTRIBUTING.md#build-assembly-and-ci-policy). Workers use capped compile checks and named targeted tests, and park requires a test receipt in each changed crate at the exact tip. The worker pre-push hook runs the no-build fast rows; lane CI runs them once, while `CAS_LANE_SLIM=true` skips broad Rust for factory pushes and runs only the train's impact-selected tests, own modules first. The merge queue is the only gate to `main` and proves the final tree, including the Rust suite and Commander journeys when web inputs change. Use a local `ASSEMBLY_PROOF` only when the close gate accepts that fallback; it covers Rust build/suite, not journeys. Do not add lane-merge or assembly journey checks, and park reads receipts but never runs checks. For web changes, workers and QA run vitest, tsc and affected journeys; the hook denies worker/reviewer `--full` or unfiltered Playwright. Keep `panic = "unwind"`.
- Module/store/hook work: [ARCHITECTURE](cas-cli/docs/ARCHITECTURE.md); navigation: [.claude/CODEMAP.md](.claude/CODEMAP.md).
- CLI/MCP/migration/skill changes: [CONTRIBUTING](cas-cli/docs/CONTRIBUTING.md).
- Bug diagnosis: trace the real handler or data and cite the confirming line or reproduced behavior before fixing; use `cas-diagnosing-bugs` for the loop.
- CAS runtime, verifier, hook, factory or builtin-skill bugs belong here, including incidents in downstream projects. Create an in-repo fix task.
- Releases and harness diaries: publication to Slack is mandatory; read [RELEASE_SLACK_RUBRIC](docs/RELEASE_SLACK_RUBRIC.md) before preparing either.
