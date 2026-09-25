---
name: cas-html-reports
description: Use when producing a human-readable report or analysis that must outlive the conversation, including investigations, audits, decision briefs, benchmarks, post-mortems, or executive updates; not published version releases (cas-release-report).
metadata:
  managed_by: cas
---

# Reports ship as HTML, and the HTML shows the argument

**Markdown is the source of truth. HTML is the human review surface. The hero is a figure.**

Every report exists twice: a markdown file that holds the words and numbers (diffable, greppable,
what future agents read) and a single-file HTML artifact beside it that a human understands in the
first three seconds. The HTML is always generated *from* the markdown, never the reverse, and both
land in the same commit. It renders with **no network, no build step, and no external files**.

A report is not a styled document. It is one argument, made visible: the reader sees the claim as a
figure above the fold before reading a sentence of prose. `cas-ui-craft` owns the craft vocabulary
this skill consumes — the concept brief, the form vocabulary, and the critique rubric. Read it once;
this skill tells you where each of those lands in a report.

## The workflow

1. **Write the markdown first.** `docs/<area>/YYYY-MM-DD-<topic>.md`. Get the analysis right in plain
   text before any presentation decision. If the markdown is weak, the HTML is decoration on nothing.
2. **Pick your cell**: report type × audience, from `references/report-types.md`. The cell fixes the
   required sections, their order, and the **hero figure** the type owes the reader.
3. **Write the concept brief** (`cas-ui-craft` step 1) and commit it beside the markdown as
   `<basename>.brief.md`, from `cas-ui-craft/references/concept-brief.md`: the single idea; the hero
   form and the reason its shape is the claim; the emotional register; one distinctive move; what is
   deliberately omitted. A brief whose hero is "a paragraph", "a row of numbers", or "a table of
   everything" is rejected before rendering: the hero shows the argument, it does not summarize the
   document. Choose every form from `cas-ui-craft/references/form-vocabulary.md`, the one form table,
   and write the reason; "a table" needs a reason as much as anything else does.
4. **Render the HTML** beside the markdown: same directory, same basename, `.html`. Obey
   `references/technical-contract.md` (one file, progressive enhancement, accessibility, print,
   provenance, design language) and `references/presentation-rules.md` (encodings, scales, numbers).
   Chart construction follows `cas-dataviz`.
5. **Score it with the `cas-ui-craft` rubric** (`references/critique-rubric.md`) and append the
   table to the brief under `## Critique`. A report ships only when it meets that rubric's floor,
   with its visual-QA receipt, a print preview, and a JS-disabled reload as evidence; then run
   `references/review-checklist.md`. A grep for expected tags is not a review.
6. **Commit all three files together**: markdown, concept brief, HTML. An HTML artifact without its
   markdown source is a provenance failure; a markdown report whose HTML is stale is worse than no
   HTML at all; a rendered report without its brief cannot be critiqued.

## What counts as a report

A deliverable is a report when **all three** hold:

1. It is a **written conclusion**, not raw output — you analyzed something and are stating what you found.
2. It is **durable** — committed to the repo, meant to be read after this session ends.
3. It has a **reader who was not in the room** — a supervisor, an operator, a client, an executive,
   or a future agent with none of your context.

Canonical cases: investigation and diagnostic write-ups, metrics and mining analyses, audits, decision
briefs, comparisons and benchmarks, incident post-mortems, status summaries (a published version release uses `cas-release-report`), financial
reports, executive and board updates, product and feature showcases, system explainers, client
deliverables, research and market analyses.

## When HTML is NOT required

Do not reach for HTML when the answer is short enough that formatting is overhead. An HTML artifact
for these buries a two-sentence answer under 300 lines of markup:

- **Chat answers.** A reply in the conversation stays prose, even a long one.
- **Task notes, progress notes, commit messages, PR descriptions.** These have their own homes.
- **A short prose answer written to a file** — a three-paragraph decision record with no numbers, no
  comparison, and no structure is markdown-only. Commit the `.md` and stop.
- **Machine-consumed output** — JSON, JSONL, CSV, logs, fixtures.
- **Living documentation** — READMEs, architecture docs, runbooks, skills.

Rule of thumb: if the report has **no table, no comparison, no time series, no more than five findings,
and no numbers a reader must scan**, markdown is sufficient. If unsure, ask whether a reader would
*scan* it (HTML) or *read* it top to bottom (markdown).

## Pick your contract

Two axes, and you need both: **type** answers *what is this?* and fixes the sections, their order,
and the hero figure; **audience** answers *who leads?* and fixes what sits above the fold and how deep
the detail runs. Same data, different lead: an executive gets the decision as a figure and the ask; a
practitioner gets the evidence and the method; an external reader gets the outcome with no internal
vocabulary. Explanatory types (product showcase, system explainer) lead with a capability map or a
system flow, never a manufactured decision. The matrix is `references/report-types.md`.

## The invariant technical contract and presentation rules

Every type, audience, and domain obeys `references/technical-contract.md`: one file (No CDN, no
framework, no build step), vanilla JS only as progressive enhancement, semantic accessible HTML,
Print-ready, Provenance per figure, copyable numbers, and the design language by default — the
project's `DESIGN.md` or, without one, `design-spec/references/tokens.css`. Encodings, scales, and
number formatting are in `references/presentation-rules.md`; chart construction follows
`cas-dataviz`.

## Worked examples

Each exemplar ships with a `.why.md` sidecar holding its concept brief, its critique scores, and the
decisions that make it work. Read the sidecars; open the HTML source only for the form you chose.

- `references/examples/investigation-annotated-timeline.html` — investigation, practitioner audience.
  Hero: an annotated timeline that places the regression, the deploy, and the fix on one axis.
- `references/examples/executive-variance-brief.html` — financial report, executive audience. Hero: a
  signed variance ladder against plan, with forecast hatched; the ask is the closing figure.
- `references/examples/benchmark-small-multiples.html` — comparison, practitioner audience. Hero:
  small multiples on one shared scale, with the crossover condition annotated.
- Before/after: `cas-ui-craft/references/exemplars/before-after.html` renders the same data the old
  way and this way, each scored on the rubric.

## Sources

The principles here are drawn from public bodies of work cited as attribution only; nothing from them
is vendored. See `references/sources.md`.
