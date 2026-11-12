---
name: cas-client-report
description: Use when preparing client-facing findings, analyses or recommendations as HTML/PDF; SOW completion, shipped-feature showcases and customer release notes use their specialized report skills.
metadata:
  managed_by: cas
---

# Client report

Prepare a report for the operator to review and send. Drafting a deliverable does not authorize
contacting the client or posting it. Keep client identities, engagement facts, output locations and
delivery channels in the project's profile or personal overlay, never in this shared skill.

## Route by purpose

| Request | Primary skill |
| --- | --- |
| Findings, analysis or recommendations for a client | This skill |
| SOW completion, acceptance evidence or feedback rounds | [cas-sow-delivery-report](../cas-sow-delivery-report/SKILL.md) |
| Showcase or walkthrough of shipped product capabilities | [cas-feature-report](../cas-feature-report/SKILL.md) |
| Customer-facing changes over a release or period | [cas-customer-release-notes](../cas-customer-release-notes/SKILL.md) |
| Internal analysis, audit or decision brief | [cas-html-reports](../cas-html-reports/SKILL.md) |

Specialized skills own their content order, evidence and approval workflow. Reuse the audience
and print rules here without replacing those contracts or producing a second report.

## Procedure

1. **Set the audience and authority.** Read the request and project/personal overlay for recipient,
   preparer, scope, period, delivery location and authorized actors. Name who researches/prepares
   and who contacts external parties; do not infer representation authority. Done when the brief
   records these inputs and any unknowns are conditional rather than invented.
2. **Write the source first.** Use plain language addressed to the client; explain technical terms
   on first use and lead with the outcome. Use two layers: overview with at-a-glance figures, then
   complete supporting sections. Each section opens with context before structured detail.
   Summary cards contain only a number and short title; put explanations, evidence and conditions
   below. State each fact fully once, remove redundant summary tables and cross-reference detail.
   Combine all forward actions and client requests chronologically in one section. Separate
   confirmed findings from conditions and say what evidence would resolve each condition.
   Cite every factual/legal claim to a checked source, document or saved record; group citations
   in Sources and reconcile amounts to those records. Done when every claim has support or an
   explicit condition, and no middle layer repeats partial details.
3. **Validate domain detail.** For bookkeeping entries follow
   [accounting entries](references/accounting-entries.md) before emitting any grids. Done when
   the saved journal model passes its validator; reports without entries skip this branch.
4. **Render for the reader.** Follow [DESIGN.md first](../design-spec/references/design-md-first.md)
   and the reusable [technical contract](../cas-html-reports/references/technical-contract.md).
   Write the [cas-ui-craft](../cas-ui-craft/SKILL.md) brief, then self-contained light-theme HTML with inline CSS, no external
   dependencies and no required JavaScript. Use the project's brand, high contrast, a restrained
   serif/sans hierarchy and whitespace; reflow at 375 px, with tables reflowing or scrolling.
   Use green for confirmed/money-in, amber for conditional and red for warnings, with text labels
   so color never carries meaning alone. Remove internal IDs, process/tool narration and review
   metadata from rendered content; use the approved preparer/date sign-off. Finalize the HTML
   with `cas deliverable finalize <draft.html> --audience customer --out <report.html>`.
   Done when the branded HTML works offline and with JavaScript disabled.
5. **Print and inspect.** Use Letter unless the project requires another paper size. Flow content
   across pages: keep only cards, small row groups and headings with their opening paragraph
   together; never apply `break-inside: avoid` to a whole chapter. Suppress PDF headers/footers
   to prevent timestamp, URL and local-path disclosure: use headless Chromium with
   `--no-pdf-header-footer --print-to-pdf=<report.pdf> <file://report.html>`, or Playwright
   `page.pdf({ format: 'Letter', printBackground: true, displayHeaderFooter: false })`.
   Decode the PDF and view every page image. Fix clipping, orphan headings, gaps over roughly a
   quarter-page and a sign-off stranded on its own page; verify page count, links and full values.
   Score the brief with its craft rubric. Done when it passes and both the 375 px HTML and every
   printed page have been visually inspected.
6. **Hand off locally.** Commit the Markdown, brief, HTML and PDF together in the project-approved
   location; copy the PDF to the configured pickup location if requested. Give the operator the
   files and proof, retaining source/revision evidence internally. Done when the review copy is
   ready; publish only under the operator's explicit delivery authorization.
