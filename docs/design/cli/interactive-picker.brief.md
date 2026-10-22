# Shared interactive picker style contract

| Field | Contract |
| --- | --- |
| First two lines | The choice title precedes the rows; the cursor identifies the current choice and completion names the selected label or says cancelled. |
| Scannable | All rows and status tags use the terminal default foreground; the cursor and bold selected label distinguish focus without relying on color. |
| Readable | Help and inactive rows remain at full contrast; styled UTF-8 keeps its glyphs while C/plain keeps ASCII marks and separators. |
| Machine output | This interactive picker has no JSON protocol; NO_COLOR emits no SGR while retaining cursor interaction, and line-based confirm/select is unchanged. |
| Omitted | No fixed palette colors or dim text; navigation, padding, labels and status words are preserved. |

## Style decision

A fixed ANSI foreground depends on the user's palette and can disappear on light,
dark or Solarized backgrounds. The shared picker therefore inherits the terminal's
foreground for every mark and word. Styled mode adds bold only to the title,
selected row and accepted title. Plain mode and NO_COLOR emit no SGR, including
resets, while cursor movement and clearing remain available inside the TTY.

The approved UTF-8 snapshot intentionally replaces the cas-ba0e color bytes.
Locale glyph selection and keyboard actions retain their existing contract.
The per-tag color input is removed so callers communicate status in words.

## Critique

terminal-qa: PASS picker-select · 11 runs · 0 fail · 0 warn · 0 allowed · <artifacts>/cas-31ac/terminal-qa/select/report.json

terminal-qa: PASS picker-cancel · 11 runs · 0 fail · 0 warn · 0 allowed · <artifacts>/cas-31ac/terminal-qa/cancel/report.json

| Dimension | Score | Evidence |
| --- | ---: | --- |
| Hierarchy | 4 | The choice title and cursor identify the action; completion names Alpha or cancelled. |
| Fit | 4 | Row labels and status words align at both widths; help remains within 44 cells. |
| Craft | 4 | Selection uses bold plus a cursor; inactive rows and help retain full foreground contrast. |
| Theme safety | 5 | Both four-palette reports pass without exceptions; all six raw NO_COLOR cases contain zero SGR. |
| Machine contract | 4 | No JSON protocol; raw inner-TTY transcripts are retained separately from the derived pipe-readable report. |

Scored by daring-spider-49 on 2026-10-22 after opening select/cancel HTML renders
at 80/120 on all four palettes. Candidate baafabf5963ae52f0010feb8d5f623d4f8db6f89,
cas 3.71.0 (baafabf), passed 32 raw TTY captures, including keyboard navigation,
wraparound, Escape/Ctrl-C cancellation and the kill/attach session item builders.
The real-build ledger is in cas-31ac/LEDGER.md under the task artifact directory.

The existing four plain-text captures remain byte-identical. The UTF-8 style JSON
now pins default foreground and bold rather than inherited fixed-color bytes.
Absolute cursor moves are projected to line boundaries only for terminal-qa;
no TTY text, glyph or SGR is rewritten. The outer pipe profile is a plain derived
report of a real inner TTY; it does not promise a non-TTY picker protocol.

Kill/attach walk inputs are isolated session metadata fixtures. Attach eligibility
uses the owned driver PID; q cancels before any connection or kill. No factory or
server is started. Selection fixtures remove the chosen directory immediately
before acceptance to stop after the summary, before factory launch. Full command
transcripts retain that intentional post-selection error.
