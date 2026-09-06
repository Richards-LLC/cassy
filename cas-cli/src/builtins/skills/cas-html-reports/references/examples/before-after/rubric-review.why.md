# Before / after — the 2026-09-06 model lane rubric review

Same markdown source, same numbers, same author. `rubric-review-before.html` is the render that
shipped first (including the model intelligence, cost and efficiency section added the same day);
`rubric-review-after.html` is the same brief rendered under this skill's contract with a committed
concept brief (`rubric-review.brief.md`). Type: decision brief. Audience: operator.

## What the before version did

- Hero: a gradient header, a two-sentence verdict paragraph, and a **five-card KPI row**. The cards
  summarized the document; none of them showed the argument.
- The rubric table and the evidence table were two separate tables the reader had to reconcile.
- The seven failures were a **card grid**, detached from the lanes they indict.
- Two absolute bars for deliveries and send-backs; status encoded by colored tags.
- Generic dark dashboard theme (system sans, blue accent, rounded panels), no design language.
- Score: distinctiveness 2, fit to argument 2, hierarchy 3, craft 3, accessibility 3. Below the floor.

## What the after version does

- **Hero is a ledger**: one row per lane, *reputation says* beside *data says*, with a verdict stamp
  in the gutter (CONSISTENT / CONTRADICTED / UNTESTED / UNUSED). The argument — the rubric contradicts
  the evidence lane by lane — is the first thing on the page, above the fold at 1280×800 and stacked
  cleanly at 390×844.
- The seven failures are **marginal notes** anchored to the rows by number.
- Evidence is **small multiples on one shared 0–20 scale**, so the standard lane's height against
  the others is the finding; release latency is two panels on one 0–5 h scale.
- The decision is the **closing figure**: options A and B drawn as the heavy lane's primary → fallback
  edge, B stamped RECOMMENDED with the promotion rule as its annotation.
- Petrastella design language, default tokens: serif display for the title and the stamps, system
  sans body, tabular mono for numbers; semantic roles (`--verdict`, `--evidence`, `--warning`,
  `--action`) declared once; light and dark; print stylesheet lets the ledger break by row.
- Every figure has a table twin; every number keeps its source; the markdown is unchanged.
- Score: distinctiveness 4, fit to argument 5, hierarchy 4, craft 4, accessibility 4. Meets the floor.

## The lesson

Both renders contain the same information. The before version asked the reader to assemble the
argument from five numbers, two tables, and seven cards. The after version chose one form that *is*
the argument and let everything else serve it. The design language did not make it better; the
concept brief did. The design language made the choice look like it belonged to a house.
