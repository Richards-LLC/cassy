# Root help wordmark

| Field | Contract |
| --- | --- |
| First two lines | Root help identifies Cassy, then offers its command usage; this help entry does not invent a health verdict. |
| Scannable | Keep the existing compact UTF-8 wordmark on styled terminals, with a single ASCII Cassy label on plain or non-UTF-8 destinations. |
| Readable | Command descriptions and option usage remain the existing root help, fitted to 80 and 120 columns. |
| Machine output | Help is usage text even with --json; it never promises a JSON document. This change adds no machine fields. |
| Omitted | Block and box glyphs are omitted in C, legacy non-UTF-8, pipe and NO_COLOR output; no new banner or decoration replaces them. |

Use the existing locale policy (`LC_ALL`, then `LC_CTYPE`, then `LANG`) and
the existing output-mode detection. Limit the change to wordmark selection.

## Critique

Pending executed locale regression and supervisor-candidate terminal captures.
