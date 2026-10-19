# CAS_ROOT override notice: C-locale fallback

| Field | Decision |
| --- | --- |
| First two lines | Preserve the existing warning that names both stores and the CAS_ROOT winner, followed by the existing one-command escape. |
| Scannable | Use the existing warning mark and dash in UTF-8, and [WARN] plus -- in non-UTF-8 locales. |
| Readable | Preserve both full path spellings and the explanation of why factory workers rely on this precedence. |
| Machine output | The notice remains stderr-only and at most once per process; command stdout and its JSON shape stay unchanged. |
| Omitted | No new routing, truncation, precedence or file mutation is introduced by this punctuation fix. |

## Critique

Per supervisor4200315, actual terminal cells and critique remain pending for the
assembled v50 flow walk. Source tests are not real-build terminal captures.
