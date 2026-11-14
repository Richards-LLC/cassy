# Supervisor brief for a report

Load `cas-html-reports` and `cas-ui-craft` before briefing or relaying a report. Copy this template into the task description of any report task. Fill in every `<field>`. Do not brief a report from memory. The workflow, the floor and the gate below are the contract, and a brief that drops them gets a report that only passes visual QA.

## Template

```markdown
Report task: load `cas-html-reports` and `cas-ui-craft` before writing anything.

Reader: <who reads it first, by role, e.g. "the operator and the client's owner">
Cell: <type from report-types.md> × <executive | practitioner | external>
Question: <the one question the report answers, e.g. "Is the factory faster than last month?">
Originating plan: <path/link, revision and section; use the original request when there is no plan>
Goal: <what that plan or request set out to establish>
Success criteria: <the original criteria, thresholds, population and time window, with their source>
Additional analysis: <any author-added tests or stricter thresholds, labelled separately from plan criteria>
Decision it supports: <what the reader will do differently after reading it>
Sources: <the data and where it lives; numbers come from these, not from memory>
Leave out: <what stays in the markdown or a collapsed section, e.g. "case-by-case rows: aggregate
them; at most the three cases that change the answer appear on the page">

Workflow (cas-html-reports, in order; each file is committed):
1. `docs/<area>/<date>-<topic>.md` first: the analysis in plain text, judging the originating
   goal against its success criteria before choosing the verdict. Label extra bars separately.
2. `<basename>.brief.md` from cas-ui-craft concept-brief.md, with `Audience: <audience>` on its
   own line and the plan alignment record from cas-html-reports review-checklist.md section 0.
3. `<basename>.html` beside it, wearing the project's DESIGN.md (`cas deliverable brand`); system
   fonts, no network, one `data-verdict` element (the answer, at most 40 words) before any section.
4. The cas-ui-craft rubric table under `## Critique` in the brief: distinctiveness, fit and
   hierarchy at least 4, no dimension 0, with visual QA (`visual-qa.mjs --strict`) as the craft
   evidence.
5. Height: at most 4,000 px at 1280 px for an executive or external reader, 8,000 px for a
   practitioner; depth goes into collapsed `<details>` or the markdown.
6. Business readers see no task ids and no internal vocabulary (workers, lanes, file paths, PR numbers).

Return: the three paths, the plan alignment record, the critique table, and screenshots at 1280×800 and 390×844.
The close gate (G7) enforces report form; the author and supervisor also review plan alignment.
```

## Fields that fail

- **"List every case", "report all the metrics" or "one row per X" as the body.** Each one invites
  the dump the operator rejects. Name the question and the decision instead. Aggregates and the few
  cases that change the answer go on the page; everything else goes in the markdown.
- **A content spec without a cell.** Without type × audience, the worker guesses who leads and
  how deep the page runs.
- **Restating the technical contract from memory.** Paste the workflow block as written. A
  re-brief that allows web fonts, skips the markdown or drops the critique contradicts the skill.

## Before relaying the report

First check the originating goal, criteria, evidence and plan verdict using
[review-checklist.md section 0](review-checklist.md#0-plan-alignment-concept-brief-and-critique).
An added author threshold cannot silently turn a met plan into a failed one; keep any further
recommendation separate. Missing or contradictory alignment returns to the author before form review.
Then open the 1280 px and 390 px screenshots yourself and score the rubric's three floor dimensions.
A visual-QA PASS proves the page renders. It does not prove the report is clear, branded or short.
Relay the report only when your scores and the worker's critique both meet the floor.
