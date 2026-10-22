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

Pending exact-revision real-TTY terminal captures; no inherited contrast or
NO_COLOR exceptions are allowed for cas-31ac.
