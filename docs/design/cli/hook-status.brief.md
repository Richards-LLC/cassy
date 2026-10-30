# Hook status: terminal-safe shared heading

| Field | Decision |
| --- | --- |
| First two lines | Identify Claude Code hook status, then begin the registration rows; this repair retains the existing command hierarchy. |
| Scannable | Eleven aligned event rows include PostModelSwitch, each with an explicit configured/not configured value. |
| Readable | The shared H1 uses bold terminal foreground, with Unicode rules only in a UTF-8 styled terminal and ASCII rules otherwise. |
| Machine output | `--json` remains one object of registration booleans (`configured`, `session_start`, `session_end`, `post_model_switch`, `stop`, `subagent_stop`, `post_tool_use`, `pre_tool_use`, `user_prompt_submit`, `permission_request`, `notification`, `pre_compact`); no banner or ANSI. Missing settings retain the existing `configured`/`reason` object. |
| Omitted | No new summary, hook details, settings mutation, JSON fields, or palette changes; this task repairs shared H1 contrast and locale handling. |

Design source: repository DESIGN.md, last design revision b43940e4321be59e122839aa9a9577e9755b2e33; terminal typography follows cas-cli-craft's default foreground policy.
