# Exemplar: doctor-style report

A checks report (`cas doctor`). Dozens of checks, most healthy; the reader wants the ones
that are not, each with one thing to run.

## Brief

| Field | Sentence |
| --- | --- |
| First two lines | Healthy or not, how many findings, and which store and version this is about. |
| Scannable | One line per healthy group with its marks; one row per finding under its group. |
| Readable | Each finding's cause (wrapped under a hanging indent) and its remedy command. |
| Machine output | `--json`: one array of `{name, status, group, message, remediation, duration_ms}`. |
| Omitted | Per-check timings, the slow-phase table, and every instance of a repeated finding: `--verbose`. |

## Render at 80 columns

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
                                 root at /tmp. Remove the stale registration w…
Config        ✓ config file  ✓ hooks  ✓ skills
Integrations  ✓ claude  ✓ codex

21 ok · 2 warnings · 0 errors · 1.4s · cas doctor --verbose for timings and full messages
```

## Annotations

1. **Verdict** — the count of findings is the word (`2 warnings`), the count of healthy checks
   is the detail. `healthy` replaces it when nothing is wrong; `N errors` outranks warnings.
2. **Healthy groups collapse** to one line each, marks wrapped at the column, names in the
   default foreground. Twenty-one checks cost seven lines.
3. **Findings expand** under their group with the mark coloured, the name bold, the cause in a
   hanging-indent column. A token wider than the column (the 90-character scope id) is cut
   with `…`; the receipt names `--verbose` as the escape.
4. **Repeats collapse** — fourteen identical `registered project roots` findings became one row
   with `×14`. Before, they were fourteen seven-line paragraphs: 98 lines for one fact. A cause
   or remedy stops at three lines with `…`; `--verbose` prints it whole.
5. **Remedies** are commands on their own line under `→`, wrapped at word boundaries with a
   hanging indent; the old render printed the 170-character remedy unwrapped.
6. **Receipt** keeps the exact `N ok · N warnings · N errors · time` grammar scripts already
   grep for, and adds the escape flag.

## Score

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 5 | `⚠ 2 warnings` first; remedy is the third line of each finding |
| Fit | 4 | healthy collapses and repeats collapse; the `registered project roots` cause is still a paragraph (message authoring, not rendering) |
| Craft | 4 | hanging indents hold at 80 and 120; the `×14` widens the name column for the whole group |
| Theme safety | 5 | marks only; whole-line status colour removed |
| Machine contract | 5 | `--json` is one array; the human render derives from the same checks |
