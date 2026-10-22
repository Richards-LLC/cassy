# Init wordmark and frame

| Field | Contract |
| --- | --- |
| First two lines | Init identifies Cassy before presenting the existing configuration choice; this locale fix preserves the wizard's entry hierarchy. |
| Scannable | Styled UTF-8 terminals retain the compact six-row splash and frame; C, legacy, pipe and NO_COLOR destinations receive the Cassy label and ASCII rules. |
| Readable | The existing welcome description and configuration choices keep their wording, indentation and 80-column fit. |
| Machine output | `cas init --json` retains its existing result document and bypasses this human splash; this change adds no JSON fields or status codes. |
| Omitted | Decorative blocks, box glyphs and bullet/arrow marks are omitted from non-styled or non-UTF-8 output; user-supplied Unicode content is retained. |

Use the same styled-output plus UTF-8 predicate as root help. Keep the raw
clap command's default banner ASCII so alternate help entry paths cannot skip
the locale policy. Limit changes to wordmark selection and init's text writer.

## Critique

Pending exact-build terminal captures at 80/120 columns in UTF-8 and C locales.
