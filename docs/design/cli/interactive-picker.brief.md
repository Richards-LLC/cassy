# Shared interactive picker locale fallback

| Field | Contract |
| --- | --- |
| First two lines | The existing title and selected item identify the choice; locale changes only the decorative marks and navigation punctuation. |
| Scannable | C/plain uses `>`, `+`, `x` and `up/down` with ASCII separators; styled UTF-8 keeps its exact cursor, completion marks and help bytes. |
| Readable | Navigation, enter-to-select and cancellation stay visible at 80/120 columns. User labels and tags are preserved verbatim. |
| Machine output | The picker has no JSON protocol; line-based confirm/select behavior is unchanged. Shared supports_styled_glyphs selects the decoration. |
| Omitted | This change adds no prompt, option, animation, color policy or layout. |

Terminal proof will exercise an actual supervisor-built candidate picker in a
TTY at 80 and 120 columns, alongside the locale/mode renderer regression.
