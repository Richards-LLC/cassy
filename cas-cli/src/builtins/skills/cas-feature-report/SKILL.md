---
name: cas-feature-report
description: "Use when asked for a feature report, showcase or walkthrough of shipped work ('feature report for X'): one HTML/PDF in the target project's brand with real before/after views; analyses and audits are cas-html-reports."
metadata:
  managed_by: cas
---

# Feature report

A feature report shows a reader what shipped, who it helps and how to use it,
with real before/after views of the product. It is one HTML file and its PDFs,
in the brand of the project the feature belongs to.

This skill owns only the report's content: the
[contract](references/report-contract.md) and the
[template](references/report-template.html). Everything else is reused:

- `cas deliverable brand` and `cas deliverable finalize`: brand, inlining,
  size budget, audience hygiene, A4 and Letter print and the PDF check. This
  skill carries no brand, capture, inline or print code of its own.
- `cas-html-reports`: markdown first, the technical contract and the
  presentation rules.
- `cas-ui-craft`: concept brief, critique rubric and visual QA.
- `cas-qa-craft`: evidence from the real build and honest labels.

Nothing reaches a client or a channel without the operator's explicit go.

## Inputs

| Input | Source |
| --- | --- |
| Feature | The request, plus the tasks, PRs or release entries that shipped it |
| Target project | The project the feature belongs to, which may not be this checkout ([DESIGN.md first](../design-spec/references/design-md-first.md) §1) |
| Audience | `customer` when the request names a client, a customer or a person outside the team; `internal` otherwise |
| Example views | The delivering tasks' QA before/after pairs, when they exist |

## Steps

1. **Name the target project and the audience.** For a customer report, the
   client's name comes from the request or the project's
   `docs/deliverables/PROFILE.md`; never guess one. Done when both are
   written in the brief.
2. **Confirm the brand.** Follow
   [DESIGN.md first](../design-spec/references/design-md-first.md), then run
   `cas deliverable brand --root <project> --audience <audience>`. A customer
   report never wears the default brand: on exit 1, resolve the named source
   with the operator or `design-spec` before writing anything. Done when the
   receipt names the project's own `DESIGN.md`.
3. **Establish what shipped.** Read the delivering tasks and their diffs, and
   check every claim against the delivered code or the running product. Mark
   anything not yet available as planned or not included, as the contract
   says. Done when every change has a source you have checked.
4. **Collect the Example views.** Use only complete before/after pairs from
   the delivering tasks' QA bundle (`qa/pairs/` in each task's artifacts;
   `cas deliverable pairs --task <id>` when `cas deliverable --help` lists
   it). Copy their JPGs into `pairs/` beside the draft. A change without a
   pair gets no Example view: never shoot, mock or reconstruct one here. A
   report with no pairs is valid. Done when every pair has been viewed and
   none shows a loading state, an error or personal data.
5. **Write the markdown** `docs/features/<date>-<feature-slug>.md` in the
   project the report is committed to, to the
   [contract](references/report-contract.md). Done when every change appears
   exactly once.
6. **Brief, render and finalize.** Write the `cas-ui-craft` brief beside the
   markdown. Fill a copy of the [template](references/report-template.html)
   from the markdown, keeping its brand markers, then run
   `cas deliverable finalize <draft.html> --root <project> --audience <audience> --out <final.html> --pdf a4,letter --check-dir <artifacts_dir>/pdf`.
   Exit 1 names each finding; fix the source, not the output. Done when it
   exits 0.
7. **Critique and view every page.** Score the final HTML with the
   `cas-ui-craft` rubric and view every page PNG under `<artifacts_dir>/pdf`
   as `cas-release-report/references/pdf.md` describes. Done when visual QA
   passes and you have viewed every page.
8. **Commit and record.** Commit the markdown, brief, HTML and both PDFs in
   one commit, and record the brand receipt's `design:` line in the task.

## Boundaries

`cas-html-reports` owns analyses, audits and decision briefs.
`cas-release-report` owns version releases, `cas-release-notes` owns merge
announcements and `cas-sow-delivery-report` owns statement-of-work
deliveries. This skill does not grant permission to post.
