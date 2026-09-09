# `cas release report`

`cas release report <version>` assembles a release-report Markdown source from
the local Keep-a-Changelog section, a matching draft in
`docs/release-notes/`, configured GitHub issue/release data, and matching
receipts under `~/.cas/artifacts/release/`. It then invokes the installed
`cas-release-report` builtin renderer to create a standalone HTML file.

```bash
cas release report 3.20.0
cas release report v3.20.0 --out docs/release-reports --pdf
```

The command writes `v<version>.md` and `v<version>.html` under `--out`; `--pdf`
also writes `v<version>.pdf` after rendering both A4 and Letter through
Playwright and retaining the A4 artifact as the committed PDF. A missing source
is recorded as unavailable in the report so the report never invents evidence.
Set the project repository before collecting GitHub evidence:

```bash
cas config set issues.repo owner/name
```

An existing Markdown source is authoritative and is never overwritten during a
normal rerun. Pass `--refresh-sources` after correcting source evidence to
regenerate it. `--json` prints a single stable result object containing output
paths, counts, ordered theme rows, warnings, retrieval time, and release timing.

The source follows the fixed `cas-release-report` skeleton: Change map, Release
at a glance, What you can do now, Under the hood, Fixes ledger, Install, and
Evidence and scope. Issue classification checks labels first and then the
documented keyword table; unknown issues stay in an explicit `Unclassified`
row for human correction before rendering.
