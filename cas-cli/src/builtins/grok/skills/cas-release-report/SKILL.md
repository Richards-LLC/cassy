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
   CHANGELOG section, release-notes draft, closed issues and publication receipts;
   writes `docs/release-reports/v<version>.md`; renders `.html` through this
   skill's [scripts/render.py](scripts/render.py) and
   [references/template.html](references/template.html); and prints the A4
   `v<version>.pdf`. Read its warnings: missing evidence is named as unavailable.
3. Edit the Markdown against the source contract in
   [references/exemplar.md](references/exemplar.md): every user and developer
   change as Was → Now, verified closures counted separately from changelog
   entries, the counting window and latency endpoints defined, exact source URLs,
   retrieval times and full asset digests kept. Rerun step 2 to re-render; an
   existing Markdown source is kept unless you pass `--refresh-sources`.
4. Fill `v<version>.brief.md` from
   [references/brief-template.md](references/brief-template.md). The renderer takes
   project tokens from `design-tokens.json` or `docs/design/design-tokens.json`
   (roles named as in the project's `DESIGN.md`), else the bundled Petrastella
   defaults. Record which in the brief.
5. Preserve the identity: a sandstone/serif verdict, closure stitches made of
   equal-area issue dots, a quiet numbered folio and one decorative stitch at
   install. Project tokens may adapt the palette and fonts; retain the motif,
   hierarchy and section order. Keep established product-surface order across
   releases. Dots count closures, never impact or severity.
6. Keep the fixed skeleton: verdict + change map; Release at a glance; What you
   can do now; Under the hood; Fixes ledger; Install; Evidence and scope. Preserve
   the template's `data-section` markers and documented map attributes. Keep the
   source table adjacent to its figure and open without JavaScript.
7. Check the PDF with [references/pdf.md](references/pdf.md): every page back to
   PNG, bounds, links, commands and full digests.
8. Apply the brief's rubric to 1280×800 and 390×844 in both color schemes, with
   keyboard and JavaScript-disabled checks, and run the strict visual-QA script
   from the `cas-ui-craft` critique rubric. Record scores and proof paths in the
   brief; revise any clipping, contrast, overflow or half-empty pagination defect
   before delivery.
9. Commit the Markdown, brief, standalone HTML, PDF and small QA receipt under
   `docs/release-reports/`. Carry these post-publication artifacts into the next
   release-prep commit; do not rewrite the published tag. Link the report and PDF
   in the release announcement draft before the authorized Slack post.

Without the `cas` CLI, render by hand with
`python3 <skills-dir>/cas-release-report/scripts/render.py <md> --output <html> --project-root .`
(`--tokens <json>`, `--emphasis <verdict text>`), then print to PDF as pdf.md describes.

**Done when** the HTML opens offline with all source content, numbers and links;
the PDF passes pdf.md; the brief's rubric passes; and committed report/PDF paths
are recorded in the release draft. Release-notes owns Slack wording and posting
authorization; this skill prepares artifacts and does not grant it.
