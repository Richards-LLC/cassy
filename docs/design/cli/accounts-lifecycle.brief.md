# Accounts lifecycle

Design: DESIGN.md (Cassy CLI uses ASCII status words and default foreground).

| Field | Contract |
| --- | --- |
| First two lines | A mutation reports `[OK] removed/renamed/disabled/enabled` and the provider/account; remove first asks default-no confirmation with provider, email and folder. |
| Scannable | One bounded receipt follows each mutation; disabled subscriptions retain their rows and carry the word `disabled` in the picker and list. |
| Readable | Errors identify the unknown, protected, busy or colliding account and say which selection or session must change. |
| Machine output | Mutations emit one object with `action`, `provider`, `account`, `folder`; `accounts list --json` adds `flags.disabled` to each account. Prompts stay on stderr. |
| Omitted | Credentials and Keychain data never appear in receipts; login staging folders stay out of account discovery. |

A unique email or folder alias works directly. `claude:<alias>` and
`codex:<alias>` disambiguate providers. Remove hard-deletes only the named
folder; symlink targets stay in the shared base. Login runs in a hidden `.tmp`
sibling, then commits by the provider email (typed name when unavailable).
A duplicate subscription offers to reuse the existing folder; cancellation drops staging.
Distinct provider identities sharing one email receive `email-2`, `email-3`, and so on.


## Critique

terminal-qa: PASS accounts-lifecycle · 12 runs · 0 fail · 0 warn · 0 allowed

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | One mutation verdict; list starts with account and finding counts. |
| Fit | 5 | The disabled five-account fixture's 80-column capture is at most 59 cells. |
| Craft | 4 | Disabled remains an explicit word; the TTY picker also dims the row. |
| Theme safety | 5 | Default foreground and ASCII marks pass four palettes, NO_COLOR and C locale. |
| Machine contract | 5 | One JSON document; flags.disabled persists and follows rename. |

Scored by warm-crane-68 on 2026-10-27. Real fixture TTY captures exercise both
new-account picker paths through a provider cancellation; neither leaves a folder.
Capture: `accounts-lifecycle.capture.txt`. Durable transcripts and the automated
receipt live in cas-c96f's published evidence artifact.

## Recovery follow-up (cas-7450)

Remove keeps the login intact if deleting its folder fails. A later Keychain
failure names the remaining item with one copyable command on its own line:
`security delete-generic-password -s 'Claude Code-credentials-<hash>'`.
An unreadable known-repo registry refuses the mutation with `account kept` and
`Run: cas doctor`; database diagnostics stay in debug logs so the command stays
copyable. An enabled account's removal leaves the policy bytes unchanged.

New-account auth catches SIGINT and SIGTERM until its staging folder is cleaned,
including the duplicate-account question. Every login sweeps physical staging
folders older than one hour; fresh folders, directory symlinks and a still-locked
auth transaction survive. Signal death is preserved after cleanup.

The cas-7450 ledger records final candidate captures and terminal-qa separately;
the earlier critique above applies to the original lifecycle delivery.
