# `cas release report` concept brief

## First two lines

The first line says whether the report was assembled and how many verified
closures it contains; the second line gives the copyable Markdown source path.

## Scannable

The human output is a short verdict followed by source, HTML, and optional PDF
paths, then one warning row per unavailable evidence source.

## Readable

The generated Markdown carries the fixed report skeleton, source references,
theme counts, issue ledger, asset digests, and release timing; source warnings
remain in the Evidence and scope section.

## Machine output

`--json` emits one object with `version`, `tag`, output paths, `issue_count`,
`asset_count`, ordered `theme_counts`, `warnings`, retrieval time, and release
timing fields; it emits no human banner or progress text on stdout.

## Omitted

Raw GitHub payloads, issue bodies, and renderer transcripts stay out of the
terminal and live only in the generated source's evidence links or the source
repository; use `--refresh-sources` to intentionally replace a Markdown source.

## Critique

The command's final terminal QA receipt is recorded with the release-report
implementation task after the binary and a fixture renderer are available.
