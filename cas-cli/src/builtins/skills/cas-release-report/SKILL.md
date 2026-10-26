---
name: cas-release-report
description: Use after every published version release, before its Slack announcement, to produce the standalone HTML release report and verified PDF with `cas release report`.
metadata:
  managed_by: cas
---

# Release report

1. Confirm the published version, commit and release assets. An unpublished
   version remains a draft.
2. From the project root run `cas release report <version> --pdf`. It gathers the
   CHANGELOG section, release-notes draft and publication receipts, and counts
   the GitHub issues closed with the release (see exemplar.md); it waits up to
   15 minutes after the release PR merges for referenced issues still open. It
   writes `docs/release-reports/v<version>.md`; renders `.html` through this
   skill's [scripts/render.py](scripts/render.py) and
   [references/template.html](references/template.html); and prints the A4
   `v<version>.pdf`. Read its warnings: missing evidence is named as unavailable.
   A latency measured with `scripts/release-latency-receipt.sh --run-dir <dir>`
   joins with `--latency-receipt <dir>/release-latency.receipt`.
3. Edit the Markdown against the source contract in
   [references/exemplar.md](references/exemplar.md): page one leads with what
   shipped, one dot per CHANGELOG change grouped by product area, with plain
   titles. GitHub issues closed is a secondary figure counted by the generator;
   do not hand-correct it. Every user and developer change stays Was → Now. Where
   the delivering task's artifacts hold a real before and after capture of the
   same surface, add its `Before:`/`After:` pair (exemplar.md); never a lone
   after shot or a general QA render. Keep
   the exact source URLs, retrieval times and full asset digests. Rerun step 2 to
   re-render; an existing Markdown source is kept unless you pass
   `--refresh-sources`.
4. Fill `v<version>.brief.md` from
   [references/brief-template.md](references/brief-template.md). The report
   wears the brand `cas deliverable brand` resolves from the project's
   `DESIGN.md` (exemplar.md); its JSON `design` and `source` fields, also in
   the report command's `--json`, name the file and revision. Record them in the
   brief. A project with no `DESIGN.md` gets one first
   ([DESIGN.md first](../design-spec/references/design-md-first.md)).
5. Preserve the identity: a serif verdict, a change map of equal-area dots (one
   per shipped change) in named product areas, a quiet numbered folio and one
   decorative row of dots at install. The project's tokens decide palette,
   surfaces, faces and radii; retain the motif, hierarchy and section order. Keep the generator's
   area order across releases. Dots count changes, never impact or severity.
   Page one never shows a zero row or an unmeasured value: omit them, and name
   them in the counting note. Write in plain words; avoid internal terms such as
   "surface", "closure" or "verified".
6. Keep the fixed skeleton: verdict + change map; Release at a glance; What you
   can do now; Under the hood; Fixes ledger; Install; Evidence and scope. Preserve
   the template's `data-section` markers and documented map attributes. Keep the
   source table adjacent to its figure and open without JavaScript.
7. Check the PDF with [references/pdf.md](references/pdf.md): every page back to
   PNG, bounds, links, commands and full digests.
8. Apply the brief's rubric to 1280×800 and 390×844 in both color schemes, with
   keyboard and JavaScript-disabled checks, and run the strict visual-QA script
   from the `cas-ui-craft` critique rubric. Record scores and proof paths in the
   brief, and name each screenshot, JSON and page-PNG path in the QA receipt
   rather than "the task artifact directory"; revise any clipping, contrast,
   overflow or half-empty pagination defect before delivery.
9. Commit the Markdown, brief, standalone HTML, PDF and small QA receipt under
   `docs/release-reports/`. Carry these post-publication artifacts into the next
   release-prep commit; do not rewrite the published tag. Link the report and PDF
   in the release announcement draft before the authorized Slack post.

To re-render by hand, run
`python3 <skills-dir>/cas-release-report/scripts/render.py <md> --output <html> --project-root .`
(`--emphasis <verdict text>`); its brand markers stay unfilled until
`cas release report` runs, so print from that command's HTML (pdf.md).

**Done when** the HTML opens offline with all source content, numbers and links;
the PDF passes pdf.md; the brief's rubric passes; and committed report/PDF paths
are recorded in the release draft. Release-notes owns Slack wording and posting
authorization; this skill prepares artifacts and does not grant it.
