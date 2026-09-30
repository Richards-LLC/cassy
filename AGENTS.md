<!-- CAS:BEGIN - This section is managed by CAS. Do not edit manually. -->
## Cassy: tasks, memory and context

Track work and knowledge in Cassy rather than in harness-local todo lists; Cassy tasks and memories persist across sessions.
Cassy's MCP tools are `task`, `memory` and `search`, named with your harness's prefix: `mcp__cas__` in Claude Code, `mcp__cs__` in Codex, `cas__` in Grok, `cas_` in OpenCode. Call them directly.

- `task`: action=create, start, close, ready.
- `memory`: action=remember.
- `search`: action=search.

Bug routing: `cas config get issues.repo` names this project's tracker, and `issues.components.{cassy,violet,cloud}` name the Cassy, Violet and Cloud trackers. File an operational bug in the matching tracker before moving on; in the Cassy source repo itself, a Cassy bug becomes a task there. If `issues.repo` is unset, record the bug as a task note.
Release notes: when a merge reaches `staging` or `main` and docs/release-notes/RUBRIC.md exists, use the `cas-release-notes` skill and follow docs/release-notes/RUBRIC.md.
<!-- CAS:END -->

# Cassy source repository

`CLAUDE.md` imports this canonical file. Rust 1.88+, edition 2024.

- Build/check/test work: read [CONTRIBUTING](cas-cli/docs/CONTRIBUTING.md#build-assembly-and-ci-policy). Workers use capped checks and named targeted tests on clean commits; the supervisor owns full assembly. Keep `panic = "unwind"`.
- Module/store/hook work: [ARCHITECTURE](cas-cli/docs/ARCHITECTURE.md); navigation: [.claude/CODEMAP.md](.claude/CODEMAP.md).
- CLI/MCP/migration/skill changes: [CONTRIBUTING](cas-cli/docs/CONTRIBUTING.md).
- Bug diagnosis: trace the real handler or data and cite the confirming line or reproduced behavior before fixing; use `cas-diagnosing-bugs` for the loop.
- CAS runtime, verifier, hook, factory or builtin-skill bugs belong here, including incidents in downstream projects. Create an in-repo fix task.
- Releases and harness diaries: publication to Slack is mandatory; read [RELEASE_SLACK_RUBRIC](docs/RELEASE_SLACK_RUBRIC.md) before preparing either.
