# Account limit recovery

- Reader: the factory supervisor coordinating a worker whose harness has exhausted its account.
- Decision: a named replacement is queued, the switch is blocked, or no account is eligible until a reported reset.
- Surface: one durable supervisor attention message per transcript limit episode, naming the worker, old and new account aliases, reason and held task IDs. Queueing is described as queued; the existing spawn lifecycle remains the authority for launch and registration.
- Machine output: the existing recycle queue's WorkerSpec JSON carries optional `limit_recycle` metadata; migration 287 stores the episode key, immutable decision detail and optional recycle request ID. No public CLI/JSON schema changes.
- Constraints: plain text without ANSI or color dependence; no supervisor account changes. Explicit spawn accounts win, recommendation overrides inherited accounts, and no eligible recommendation retains the inherited fallback.

## Recovery contract

Automatic limit recovery validates the selected provider recipe, credentials and workspace trust before stopping the old pane. It preserves the existing worktree directory, branch and task bindings, including dirty files and unpublished commits. Manual recycle still refuses unsafe worktrees.

The replacement continues a copied Claude or Codex transcript when possible. An unavailable transcript produces an explicit fresh-task reason. The resume message includes the branch, uncommitted files and unpublished commits with counts; a branch without an upstream compares against remote refs.

The key contains factory session, worker name and first evidence, so resuming a copied refusal cannot trigger another switch after re-registration. Refusals and no-eligible decisions persist beyond normal spawn queue cleanup.

## Evidence

The targeted tests cover both provider parsers, copied transcript bytes, continuation selectors, inherited/explicit/fallback selection, dirty unpublished work, queue deduplication and rollback, supervisor refusal and the existing-database migration. Real subscription exhaustion is not induced: the live continuation row remains NOT EXERCISED because no account can safely be driven to its limit.
