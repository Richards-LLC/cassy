# Shared interactive picker locale fallback

| Field | Contract |
| --- | --- |
| First two lines | The existing title and selected item identify the choice; locale changes only the decorative marks and navigation punctuation. |
| Scannable | C/plain uses `>`, `+`, `x` and `up/down` with ASCII separators; styled UTF-8 keeps its exact cursor, completion marks and help bytes. |
| Readable | Navigation, enter-to-select and cancellation stay visible at 80/120 columns. User labels and tags are preserved verbatim. |
| Machine output | The picker has no JSON protocol; line-based confirm/select behavior is unchanged. Shared supports_styled_glyphs selects the decoration. |
| Omitted | This change adds no prompt, option, animation, color policy or layout. |

Terminal proof exercised the supervisor-built candidate picker in a TTY at
80 and 120 columns. The real-build ledger is art-a50aa9a3; captures, exact
tuples, scripts and receipts are in art-90fa1268.

## Critique

terminal-qa: PASS picker-select · 11 runs · 0 fail · 0 warn · 23 allowed · art-90fa1268:final-terminal-select/report.json

terminal-qa: PASS picker-cancel · 11 runs · 0 fail · 0 warn · 18 allowed · art-90fa1268:final-terminal-cancel/report.json

| Dimension | Score | Evidence |
| --- | ---: | --- |
| Hierarchy | 4 | Open project title and selected Alpha precede navigation; completion names Alpha or cancelled. |
| Fit | 4 | Choice rows stay short at 80/120; ASCII up/down help states enter/select and q/cancel. |
| Craft | 4 | Cursor and completion marks keep their padding; four styled UTF-8 PTY transcripts are byte-identical to baseline. |
| Theme safety | 2 | Inherited fixed-color contrast and NO_COLOR SGR remain under supervisor #4206464's exact-tuple exception, tracked cas-31ac. |
| Machine contract | 4 | No JSON surface changed; raw PTY evidence and derived pipe-readable report are separate, explicitly labelled. |

Opened select/cancel screenshots at both widths on all four palettes. Theme
safety is an explicitly approved inherited defect, not a claim of full style
compliance. The locale-only gate allows exactly 20/16 contrast and 3/2 NO_COLOR
SGR findings, respectively, and verifies their full tuple multisets. Any new
finding fails. Follow-up cas-31ac targets v57 epic cas-09fb.

The committed `.capture.txt` files are line projections of actual terminal
frames; raw ANSI, complete command streams, exact tuples, scripts and the
real-build ledger live in task artifacts. Accepted selection uses a deliberately
removed owned fixture to stop before factory launch. No thresholds were added.
The snapshot JSON pinned actual styled renderer bytes; the real-build walk
pinned four styled UTF-8 transcripts independently. The regression forced
crossterm color output and restored its prior cached setting on drop, including
unwinding. It explicitly exercised NO_COLOR absent and present with both locales
and both output modes; it did not inherit the harness color cache.
