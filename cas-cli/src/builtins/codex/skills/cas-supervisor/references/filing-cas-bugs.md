# Filing Cassy-system bugs

A standing directive: **file every Cassy-system bug you observe, by reflex.** Do not leave it in chat, defer it to "later", or say only "report it upstream". A Cassy-system bug you noticed but did not preserve is a bug that resurfaces.

"Cassy-system" means a defect in Cassy itself: the verifier, hooks, factory/director orchestration, MCP dispatch, the task-verifier agent, worker/supervisor prompts, or builtin skills — regardless of which downstream project surfaced it.

## Canonical routing

Choose the destination by the component that owns the defect, then file a
public-safe GitHub issue there. The project repository is configured locally;
the three Cassy component repositories have compiled defaults and can be
overridden for a fork or alternate deployment:

- **Project bug or feature:** `issues.repo` — the current project's own issue
  tracker. In cas-src, create the corresponding in-repo task.
- **Cassy-system defect:** `issues.components.cassy` — Cassy runtime, hooks,
  MCP, factory, and builtin skills. Downstream repositories consume Cassy and
  must not patch it locally.
- **MechaCassy defect:** `issues.components.mecha_cassy` — the Slack hub and
  message-delivery component.
- **Cassy Cloud defect:** `issues.components.cloud` — cloud sync, hub relay,
  pairing, and related services.

If you hit a bug during operation, file a ticket in the matching repo before moving on. Actionable requests for a Richards-LLC-controlled team belong on
that component's issue board; never write, commit, or push in that team's
checkout from this repository.
- **Receipt:** after every cross-team filing, save a Cassy memory with the issue
  URL, one-line ask, and date. Recent examples are cloud-to-Cassy GH #215 and
  Cassy-to-cloud `Richards-LLC/petra-stella-cloud#44`.

Inspect all four resolved destinations with:
`cas config get issues.repo`, `cas config get issues.components.cassy`,
`cas config get issues.components.mecha_cassy`, and
`cas config get issues.components.cloud`. Configure the project target with
`[issues] repo = "owner/repo"`, or use `cas config set issues.repo <owner/repo>`;
component overrides use the corresponding `issues.components.*` key. Do not derive
any target from a downstream git `origin`.

Before filing, check `command -v gh` and `gh auth status`. Use
`gh issue create --repo <owner/repo>` with a complete public-safe body. If `gh`
is not installed or not authenticated, preserve the report in the task or Cassy
memory, report the exact failure, and ask the operator for access or direction;
do not silently substitute a new outbound request file.

`docs/requests/` is deprecated for new outbound actionable work. Preserve its
history and read inbound `RESPONSE-*.md` files as legacy material. It remains
appropriate only for prose-heavy specifications or design documents until
cross-project task proposals ship.
