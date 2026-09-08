# Doctor hook silence diagnostic

| Field | Contract |
| --- | --- |
| First two lines | The existing doctor verdict includes a warning when recent factory turns were observed without UserPromptSubmit. |
| Scannable | One `prompt hook` row shows the number of missed prompts across recently active sessions. |
| Readable | The finding identifies UserPromptSubmit and directs the operator to restart Claude; fallback delivery continues in the current session. |
| Machine output | The existing doctor JSON check carries `name`, `status`, and `message`; no new output envelope or ANSI is introduced. |
| Omitted | Raw prompts, session identifiers, and transcript bodies stay out of the diagnostic; only bounded delivery receipts are counted. |
