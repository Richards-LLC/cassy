# Filing Cassy-system bugs

A standing directive: **file every Cassy-system bug you observe, by reflex.** Do not leave it in chat, defer it to "later", or say only "report it upstream". A Cassy-system bug you noticed but did not preserve is a bug that resurfaces.

"Cassy-system" means a defect in Cassy itself: the verifier, hooks, factory/director orchestration, MCP dispatch, the task-verifier agent, worker/supervisor prompts, or builtin skills — regardless of which downstream project surfaced it.

## Canonical routing

- **Cassy-system defect, from any repository:** file a public-safe GitHub issue in
  the configured Cassy issue repository. In cas-src, create the corresponding in-repo task; downstream
  repositories consume Cassy and must not patch it locally.
- **Actionable request for a Richards-LLC-controlled team:** file directly on
  that team's repository issue board (for example,
  `Richards-LLC/petra-stella-cloud`). Never write, commit, or push in that
  team's checkout from this repository.
- **Receipt:** after every cross-team filing, save a Cassy memory with the issue
  URL, one-line ask, and date. Recent examples are cloud-to-Cassy GH #215 and
  Cassy-to-cloud `Richards-LLC/petra-stella-cloud#44`.

Configure the target with `[issues] repo = "owner/repo"`; inspect it with
`cas config get issues.repo` and set it with `cas config set issues.repo <owner/repo>`.
Do not derive the target from a downstream git `origin`.

Before filing, check `command -v gh` and `gh auth status`. Use
`gh issue create --repo <owner/repo>` with a complete public-safe body. If `gh`
is not installed or not authenticated, preserve the report in the task or Cassy
memory, report the exact failure, and ask the operator for access or direction;
do not silently substitute a new outbound request file.

`docs/requests/` is deprecated for new outbound actionable work. Preserve its
history and read inbound `RESPONSE-*.md` files as legacy material. It remains
appropriate only for prose-heavy specifications or design documents until
cross-project task proposals ship.
