# cas update finish accounts

| Field | Contract |
| --- | --- |
| First two lines | Each account receipt states finish updated, current, or preserved, with counts before details. |
| Scannable | Named accounts and shared bases each have one attributed finish receipt after the default profiles. |
| Readable | Conflicts identify both personal and profile paths; discovery warnings name the provider and explain that other sync continues. |
| Machine output | The existing JSON document adds finish_accounts (path, updated, preserved, removed, warnings) and account_discovery_warnings; default-profile counters retain their meaning. |
| Omitted | Unrelated account configuration and unchanged skill inventories are not listed for finish-only refresh. |

Personal Codex finish takes precedence. A hash-proven pristine Cassy copy is
removed; dual-custom conflicts preserve both paths for operator review. JSON
retains full paths and causes; human rows shorten long paths with an explicit
`cas update --user --json` escape.
