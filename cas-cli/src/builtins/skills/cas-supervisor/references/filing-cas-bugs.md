# Filing Cassy-system bugs

Report a Cassy-system bug you observe once, when you observe it, rather than leave it in chat: an unrecorded bug resurfaces, and a duplicate splits its evidence. Never file per refusal, apology or workaround.

"Cassy-system" means a defect in Cassy itself: the verifier, hooks, factory/director orchestration, MCP dispatch, the task-verifier agent, worker/supervisor prompts, or builtin skills, whichever downstream project surfaced it.

## Canonical routing

File a public-safe GitHub issue in the repository of the component that owns the defect. The project repository is configured locally; the three Cassy component repositories have compiled defaults that a fork can override:

- **Project bug or feature:** `issues.repo`, the current project's tracker.
- **Cassy-system defect:** `issues.components.cassy`: runtime, hooks, MCP, factory and builtin skills. Downstream repositories consume Cassy and must not patch it locally.
- **Violet defect:** `issues.components.violet`: the Slack hub and message delivery.
- **Cassy Cloud defect:** `issues.components.cloud`: cloud sync, hub relay, pairing and related services.

Find or file the ticket before moving on. Requests for another team go on that component's board; never write, commit or push in that team's checkout. After a cross-team filing, save a Cassy memory with the issue URL, the one-line ask and the date.

Inspect the destinations with `cas config get issues.repo`, `cas config get issues.components.cassy`, `cas config get issues.components.violet` and `cas config get issues.components.cloud`. Configure the project target with `[issues] repo = "owner/repo"` or `cas config set issues.repo <owner/repo>`; component overrides use their `issues.components.*` key. Never derive a target from a downstream git `origin`.

For a Cassy-system defect, call `system action=report_cas_bug` with the verbatim refusal as the first line of `actual`: it records the report in the local ledger and reaches GitHub only when the operator reports it or the same refusal recurs five times in seven days; then it matches the refusal's gate code and normalized text against open issues and tasks, adds the evidence and +1 to a match, and files only when none matches. For other components, find first: search open issues (`gh issue list --repo <owner/repo> --state open --search "<refusal text>"`) and open tasks; on a match, comment the new evidence and +1 there. Only when nothing matches, check `command -v gh` and `gh auth status`, then run `gh issue create --repo <owner/repo>` with a complete public-safe body. Without a working `gh`, keep the report in the task or a Cassy memory, report the exact failure and ask the operator for access; do not write a new outbound request file instead.

`docs/requests/` is deprecated for new outbound work. Keep its history, read inbound `RESPONSE-*.md` files as legacy, and use it only for prose-heavy specifications until cross-project task proposals ship.
