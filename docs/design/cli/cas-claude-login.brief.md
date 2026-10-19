# `cas claude login` path announcement

| Field | Contract |
| --- | --- |
| **First two lines** | The opening line names the Claude profile directory being used, followed by any setup notice. |
| **Scannable** | Login prints one short path announcement before it runs Claude authentication. |
| **Readable** | The profile location uses `~` for a path under `HOME`, which keeps the command line legible and shell-resolvable. |
| **Machine output** | `cas claude login` has no `--json` output; the login child still receives the full path through `CLAUDE_CONFIG_DIR` and `CLAUDE_SECURESTORAGE_CONFIG_DIR`. |
| **Omitted** | The redundant absolute `HOME` prefix is omitted when the profile is inside `HOME`; paths outside `HOME` remain absolute. |
