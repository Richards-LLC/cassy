# SOW delivery report contract

The report is the `cas-html-reports` **client-facing deliverable** for the
**external** audience. That skill's technical contract, presentation rules and
provenance rules all apply. This file adds only what an SOW delivery needs.

## Sections, in order

The renderer keys on each section's `data-section` marker in
[report-skeleton.html](report-skeleton.html).

| # | `data-section` | Content |
| --- | --- | --- |
| 1 | `verdict` | One sentence in the client's terms, e.g. "Every outcome in the Analytics SOW is live on staging; one chart colour waits on your choice." Under it sits the hero (below). Under the hero: environment, capture date, round `n` of `rounds_included`, review due date |
| 2 | `outcomes` | Evidence ledger, one row per DoD requirement: `R#`, the SOW sentence quoted, status, the evidence (a shot reference or a measured fact), and for Partial, the client's decision |
| 3 | `changes` | Rounds ≥ 2 only. One before/after pair per round item, in `ROUNDS.md` order, each captioned with the client's request quoted and what changed |
| 4 | `walkthrough` | Remaining evidence shots for Met rows, grouped by the client's own feature names, one caption each saying what the screen proves |
| 5 | `decisions` | Only when Partial rows exist: each open client decision as a question with its options |
| 6 | `rounds` | Rounds ledger from `ROUNDS.md`: round, delivered, review due, items, status. Then one line saying how the next round works |
| 7 | `provenance` | Environment URL, deployed version if `version_endpoint` reports one, capture window in the client's time zone, SOW document title and date, report date, then `Markdown source:` and the report file name. `check-pdf.py` keys its footer-only last-page check on that phrase |

## Hero

The default hero is an **outcome strip**: one mark per DoD requirement, in
`R#` order, labelled with its short name. A filled mark means Met (live), a
dot inside a ring means Met (measured), and a half-filled mark with a printed
"your call" label means Partial. Its shape
carries the claim: the SOW's promises, each visibly kept. Swap it in the
brief only when the data argues for another form, for example a round-2
report whose story is the changes. In that case lead with the largest
before/after pair.

## Status vocabulary

Only `Met (live)`, `Met (measured)` and `Partial` appear. A `Gap` row never
reaches a completion report (SKILL step 3). Pair every status with its label
and a mark shape, never colour alone. A Met (measured) row says where it was
measured, and its evidence caption reads "from the report delivered on
{date}" for an artifact shot, so the client never reads an experiment as a
shipped feature.

## Status variant

When `check-sow.py gap` stops on Gap rows and the PM needs a status, produce
this variant instead of the completion report:

- The same sections, except `changes` and `rounds` are omitted unless the
  round already has items. The verdict reads "not yet complete" and names
  how many outcomes remain.
- Gap rows appear in the ledger and the hero with their owner: `Gap` is
  ours, `Gap → PM` waits on the PM. Partial rows list the drafted client
  question.
- The handoff is only handoff.md's **Status to PM** message. The completion
  message and the client message are never drafted for it.
- It goes to the PM alone. It reaches the client only when the PM decides so.

## Before/after pairs

- Each pair has the same shot id, the same viewport and the same framing.
  Stack before then after, each at full content width, on screen and in the
  PDF. Keep each image with its caption; a pair may span pages rather than
  shrink into two print columns. Each side carries a caption with its label
  (`Before`, `After`), round, environment and capture date.
- When no *before* exists, the left slot reads "Before not captured" and names
  the reason. Never leave a blank image or substitute a mock.
- Use at most one highlight per *after* image: an outline or numbered marker
  on the changed region, drawn in the brand's verdict colour as an SVG overlay
  on the image. Position it for that screenshot and check that no stroke crosses
  text; omit it when there is no clear margin. Never paint the highlight into
  the pixels, so the screenshot stays the unedited evidence.
- The caption states the change in the client's words. The image proves it.
  The caption never makes a claim the image cannot show.

## Screenshot budget

Screenshots are the evidence here, so this report embeds raster images where
other reports draw SVG. Record this exception in the brief's *Deliberately
omitted* field.

- Embed JPEG at quality 80, 1440px wide at most. `capture.mjs` writes both
  this copy and a full-resolution PNG.
- Aim for under 250 KB per image and under 8 MB per file. Above that, crop
  to the region that proves the row rather than lowering the quality further.
- Inspect the screenshot text in A4 at 100%, not only its caption. Crop the
  proving region, with identical framing for before and after, when a full
  application screen makes that text too small to read.
- A table, a number or a text claim stays HTML text, never a screenshot of
  it.

## Vocabulary and hygiene

The report goes to the client, so it carries only the client's and the SOW's
vocabulary. The HTML is generated from the markdown, so check the markdown
before rendering with `scripts/check-sow.py hygiene <report.md>`. It exits 1
and prints each line that holds a ticket label, a PR number, a branch, commit,
agent, worker, supervisor, epic, sprint or ticket word, a local or preview
URL, `TODO` or placeholder text.

The checker accepts common identifier families by default: COVID-19, SHA
hash names, UTF encodings, ISO standard numbers, GPT model numbers, and
four-digit Q1–Q4, H1–H2 or FY periods. CAS IDs and known PROJ/ENG tracker
prefixes are checked in either case; other numeric tracker prefixes use their
canonical uppercase form. A standard identifier on a line does not exempt a
tracker label elsewhere on that line.

If a hit is the client's own word (a product genuinely named "Epic"), pass
`--allow Epic` and record the reason in the brief. Other forbidden content:

- internal team or agent names;
- process narration ("after three attempts…");
- dated internal events;
- any credential or test-account identifier.

Mask client personal data in screenshots unless the client explicitly asked
to see it.

## Internal source map

For every row and caption, keep a `SOURCES.md` beside the shots in the task
artifacts directory: the requirement, the shot id, the delivering change (PR,
task) and the capture URL. It answers "where did this come from?" without
putting internal labels in the client's file. Do not commit it under the
report directory.
