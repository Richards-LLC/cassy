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
A duplicate email offers to reuse the existing folder; cancellation drops staging.
