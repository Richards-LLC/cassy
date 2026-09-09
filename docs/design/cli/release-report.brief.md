# `cas release report` concept brief

## First two lines

The first line says whether the report was assembled and how many verified
closures it contains; the second line gives the copyable Markdown source path.

## Scannable

The human output is a short verdict followed by aligned Source, HTML, and
optional PDF path rows; long paths, the remedy, and warning rows wrap under
their value or mark with an indented continuation.

## Readable

The generated Markdown carries the fixed report skeleton, source references,
theme counts, issue ledger, asset digests, and release timing; the release-note
user punch is the verdict, bold draft group labels organize Was/Now articles,
and draft-derived themes keep the change map and front matter aligned. Source
warnings remain in the Evidence and scope section.

## Machine output

`--json` emits one object with `version`, `tag`, output paths, `issue_count`,
`asset_count`, ordered `theme_counts`, `warnings`, retrieval time, and release
timing fields; it emits no human banner or progress text on stdout.

## Omitted

Raw GitHub payloads, issue bodies, and renderer transcripts stay out of the
terminal and live only in the generated source's evidence links or the source
repository; use `--refresh-sources` to intentionally replace a Markdown source.

## Critique

terminal-qa: PASS cas-release-report-cas-ca80 · 12 runs · 0 fail · 0 warn · 0 allowed · /home/pippenz/.cas/artifacts/cas-ca80/terminal-qa/report.json
| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | The verdict is line one; aligned artifact rows and one remedy follow before warning evidence. |
| Fit | 4 | Paths stay scannable as labeled rows, while only long values and warnings expand with hanging indents. |
| Craft | 4 | Source/HTML/PDF labels align, continuation rows stay within 80 cells, and the C-locale capture uses ASCII marks. |
| Theme safety | 5 | Four palettes, piped output, NO_COLOR, and LC_ALL=C all pass with marks carrying the status meaning. |
| Machine contract | 5 | The human branch uses Formatter while the existing `--json` serialization branch is unchanged; terminal QA passes the pipe contract. |
Scored by nimble-viper-86 on 2026-09-09; floor holds. The v3.22.0 run records
one verified closure (#767), the published receipt timestamp, the draft punch,
and four stable draft-derived themes in
`/home/pippenz/.cas/artifacts/cas-ca80/v3.22-run/v3.22.0.md`.
