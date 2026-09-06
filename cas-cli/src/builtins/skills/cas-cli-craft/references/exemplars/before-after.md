# Before / after: `cas doctor`

The same 23 checks, rendered the old way and this way, scored on the rubric. Captures at 80
and 120 columns on four palettes live beside the source under `docs/design/cli/captures/`.

## Before (80 columns, 248 lines; first 30 shown)

```
cas doctor · cas-src · 3.17.2
────────────────────────────────────────────────────────────────────────────────
Store         ✓ cas directory  ✓ database  ✓ schema
              ✓ factory session cas-src-lively-panther-31  ✓ tables
Indexes       ✓ legacy search index  ✓ search index  ✓ symbol index
Cloud         ✓ supervisor relay  ✓ delivery retries  ✓ canonical id
  ⚠ cloud identity metadata  foreign cloud scope(s) for project `cas-src`:
                             team_project_registered_2a57bec9-5dfa-4a8f-b711-31f
                             9aeb8d6cb_gabber-studio=2026-08-31T18:04:22.8295309
                             94+00:00,
  → run `cas cloud purge-foreign --dry-run`, then `cas cloud purge-foreign`; only after the purge, run `cas cloud sync` to re-register the current project
  ⚠ registered project roots Registered root `/tmp/.tmpdInYXx` is excluded from
                             `cas update` discovery: a disposable temp root at
                             /tmp. Remove the stale registration with `cas
                             known-repos forget /tmp/.tmpdInYXx`. If it has a
                             cloud link, run `cas cloud unlink --purge-remote`
                             from that root first; this explicitly removes its
                             remote rows without changing local files.
  ⚠ registered project roots Registered root `/tmp/.tmpwxViVG` is excluded from
                             `cas update` discovery: a disposable temp root at
  … (the same seven lines twelve more times)
21 ok · 2 warnings · 0 errors · 1.4s
```

Every line above was painted in its status colour — the whole warning paragraph in amber, every
healthy line in green — and the header in `#e4e5eb`, which on a white terminal is 1.2:1.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 2 | the command name is line one; the verdict is the last line |
| Fit | 2 | fourteen identical findings as fourteen paragraphs |
| Craft | 1 | scope ids split mid-token; the remedy line is 170 cells wide |
| Theme safety | 1 | whole-line colour; near-white header |
| Machine contract | 4 | `--json` is already one array |

Mechanical zeros: `overflow` (the remedy line), `word-split` (three scope ids), `contrast`
(header and warning paragraphs on the light palette).

## After (80 columns, 21 lines)

```
⚠ 2 warnings · 21 ok · cas-src · 3.17.2
────────────────────────────────────────────────────────────────────────────────
Store         ✓ cas directory  ✓ database  ✓ schema  ✓ factory session
              ✓ tables  ✓ entry store  ✓ memory stats  ✓ memory decay  ✓ rules
              ✓ tasks
Indexes       ✓ legacy search index  ✓ search index  ✓ symbol index
              ✓ embedding drain  ✓ code history index  ✓ embeddings
Cloud         ✓ supervisor relay  ✓ delivery retries  ✓ canonical id
  ⚠ cloud identity metadata      foreign cloud scope(s) for project `cas-src`:
                                 team_project_registered_2a57bec9-5dfa-4a8f-b7…
    → run `cas cloud purge-foreign --dry-run`, then `cas cloud purge-foreign`;
      only after the purge, run `cas cloud sync` to re-register the project
  ⚠ registered project roots ×14 Registered root `/tmp/.tmpdInYXx` is excluded
                                 from `cas update` discovery: a disposable temp
                                 root at /tmp. Remove the stale registration
                                 with `cas known-repos forget /tmp/.tmpdInYXx`.
Config        ✓ config file  ✓ hooks  ✓ skills
Integrations  ✓ claude  ✓ codex

21 ok · 2 warnings · 0 errors · 1.4s · cas doctor --verbose for timings and full messages
```

Only the marks carry colour; everything else is the terminal's own foreground and bold.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 5 | verdict first, remedies under each finding, receipt last |
| Fit | 4 | groups and repeats collapse; the cause text is still a paragraph |
| Craft | 4 | no split tokens, no overflow; the widened name column is the seam |
| Theme safety | 5 | four palettes pass; `NO_COLOR` reads identically |
| Machine contract | 5 | unchanged array |

## What changed, in the renderer

- A `verdict()` line replaced the command-name header.
- `write_report_line` stopped colouring whole lines; `mark()` colours one glyph.
- `wrap_report_text` truncates a token wider than the column instead of splitting it.
- Findings with the same name and status fold into one row with a count.
- Remedies wrap at word boundaries under a hanging indent.
- The receipt keeps the old summary grammar and names the escape flag.
