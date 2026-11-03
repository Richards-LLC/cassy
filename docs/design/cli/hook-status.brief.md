# Hook status: terminal-safe shared heading

| Field | Decision |
| --- | --- |
| First two lines | Identify Claude Code hook status, then begin the registration rows; this repair retains the existing command hierarchy. |
| Scannable | Eleven aligned event rows include PostModelSwitch, each with an explicit configured/not configured value. |
| Readable | The shared H1 uses bold terminal foreground, with Unicode rules only in a UTF-8 styled terminal and ASCII rules otherwise. |
| Machine output | `--json` remains one object of registration booleans (`configured`, `session_start`, `session_end`, `post_model_switch`, `stop`, `subagent_stop`, `post_tool_use`, `pre_tool_use`, `user_prompt_submit`, `permission_request`, `notification`, `pre_compact`); no banner or ANSI. Missing settings retain the existing `configured`/`reason` object. |
| Omitted | No new summary, hook details, settings mutation, JSON fields, or palette changes; this task repairs shared H1 contrast and locale handling. |

Design source: repository DESIGN.md, last design revision b43940e4321be59e122839aa9a9577e9755b2e33; terminal typography follows cas-cli-craft's default foreground policy.

## Critique

`terminal-qa: PASS hook-status · 12 runs · 0 fail · 0 warn · 0 allowed`

Candidate: `1380fafa41a9bdefa956e36aea6ad2f23f3c9cf5`, supervisor-built on
macOS. Captures and machine receipt live in task artifacts under
`cas-d31a/terminal-qa/report.json`; the same fixture on the baseline produced
four contrast failures and one Unicode fallback failure. The heading uses the
terminal foreground, so its color follows each palette rather than forcing teal.
All lines fit both 80 and 120 columns.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 2 (retained) | Line one remains the existing title; supervisor decision 550594 explicitly accepts title-first output for this repair. |
| Fit | 3 (retained) | Eleven individual event rows remain, including PostModelSwitch; changing the registration overview is outside this repair. |
| Craft | 4 | Heading weight is consistent in all four palettes; the existing aligned event rows remain short and unwrapped. |
| Theme safety | 5 | Four palettes at two widths, NO_COLOR, pipe and C-locale captures have zero findings; a separate LANG=C PTY capture also uses ASCII. |
| Machine contract | 5 | One JSON document retains twelve booleans; removing PostModelSwitch flips its field to false and retains the human remedy. |

Scored by the assigned worker on 2026-10-30. Contrast and locale floors pass;
the existing hierarchy and row-grain debt is disclosed, not scored as repaired.
No allowlist is used. The LANG=C probe strips the terminal's OSC background
query before checking visible text, while retaining the raw capture.
