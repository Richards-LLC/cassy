---
name: cas-sow-delivery-report
description: Use when a statement of work (SOW) is believed complete, or the client's next feedback round is done, and needs a client-branded delivery report with before/after screenshots, a round tracker and the PM handoff.
metadata:
  managed_by: cas
---

# SOW delivery report

When the developer believes a statement of work (SOW) is done, they prove it to
the client with one branded report. The report lists each Definition-of-Done
requirement with its status and screenshot evidence from the deployed app. Each
change the client asked for in a feedback round gets a before/after pair. The
project manager (PM) then receives the report and the client message to send.

This skill adds only what an SOW delivery needs and reuses the rest:

- `cas-html-reports`: markdown-first, the technical contract, the external
  audience row and the client-deliverable type.
- `cas-ui-craft`: concept brief, design language, rubric and visual QA.
- `cas-qa-craft`: evidence from the real build and honest labels.
- `cas-release-report/references/pdf.md`: the PDF check.

Nothing reaches the client or the client channel without the operator's
explicit go. Client messages carry commitments the operator owns.

`<skills-dir>` below is the harness skill directory: `.claude/skills`,
`.codex/skills` or `.grok/skills`.

## Inputs

| Input | Source |
| --- | --- |
| Delivery rules | `docs/sow/DELIVERY.md`: PM, channels, review cycle, rounds, approval threshold, environments, QA login pointer, brand sources (step 1) |
| SOW | `docs/sow/<sow-slug>/SOW.md`. Its **Outcome** section is the Definition of Done (DoD). If the SOW lives outside the repo, import it as markdown first, because the report cites it by path. |
| Gap check | `docs/sow/<sow-slug>/GAP-CHECK.md` (step 3) |
| Round | `n = 1` for the first delivery. The next bundle of client feedback opens `n + 1` |
| Brand | The `DESIGN.md` and logo asset that `DELIVERY.md` names |

## Steps

1. **Ensure the delivery rules exist.** Read `docs/sow/DELIVERY.md`. If it is
   missing, copy [references/DELIVERY-template.md](references/DELIVERY-template.md).
   Ask the operator for every `<<...>>` value. The PM, the channels, the cycle
   length, the round counts and the approval threshold are business terms, so
   do not infer them from code. Commit the file. Done when
   `grep -nE '<<[^>]+>>' docs/sow/DELIVERY.md` prints nothing.
2. **Confirm the brand source.** Open the `DESIGN.md` that `DELIVERY.md` names
   ([DESIGN.md first](../design-spec/references/design-md-first.md)).
   Compare its primary colour and font family with one live screen of the
   environment. If it is missing or disagrees, refresh it with `design-spec`
   first. A report branded from stale tokens looks foreign to the client. Done
   when the tokens you will use match the live app.
3. **Gap-check the DoD.** Write `GAP-CHECK.md` in the table format at the top
   of [scripts/check-sow.py](scripts/check-sow.py): one row per atomic Outcome
   requirement, `R1..Rn`, the SOW sentence quoted, a status, the evidence and
   the owner or open decision. Use these statuses:
   - **Met (live)**: shown on the deployed app.
   - **Met (measured)**: measured on an experiment or a delivered artifact that
     is not in the deployed app. The client must never read it as shipped.
   - **Partial**: done except for a piece only the client can supply, for
     example "X or Y for this chart?". Ask the client in the client channel.
     This question does not open a change round.
   - **Gap**: our work is missing. Finish it before the report. Write
     **Gap → PM** when the gap comes from ambiguity in the SOW itself, and
     take it to the PM first.

   Run `python3 <skills-dir>/cas-sow-delivery-report/scripts/check-sow.py gap GAP-CHECK.md`.
   Done when it exits 0. A Gap row makes it exit 1 and name the rows: the SOW
   is not done, so the completion report and the client message stop there.
   When the PM needs a status before then, produce the **status variant** in
   [references/report-contract.md](references/report-contract.md) instead.
4. **Open the round in `ROUNDS.md`.** Follow the format in
   [references/rounds.md](references/rounds.md). For `n > 1`, list every item
   from the client's bundled feedback with its estimate in hours (QA
   included), whether it is in scope, and its approval. Apply that file's
   threshold, small-SOW and PM-ping rules. Done when every item has an
   estimate, a scope call and an approval state, and every PM message those
   rules require is drafted.
5. **Write the shot list** `shots.json`, using the format in
   [scripts/capture.mjs](scripts/capture.mjs)'s header:
   - one shot per Met or Partial requirement whose evidence is visual;
   - one *after* shot per round item.

   Capture every environment shot in one round from one environment: staging
   when `DELIVERY.md` lists it, otherwise production. A Met (measured) row's
   evidence may be a report already delivered to the client: give that shot
   `"source": "artifact"` and a `file://` URL, and caption it "from the report
   delivered on {date}", never with the environment. Take each *before* from
   the first source on this list that has one:
   1. the same shot id in the previous round's shot set, which gives
      identical framing;
   2. production, while the change is live only on staging;
   3. the client's own screenshot from their feedback.

   If none applies, label the pair `before not captured`. A mocked or
   reconstructed *before* misstates history. Done when every visual
   requirement and every round item has a shot id, and every round item has a
   *before* source or that label.
6. **Capture headless with the project's QA login.** Run
   `npm exec --yes --package=playwright -- node <skills-dir>/cas-sow-delivery-report/scripts/capture.mjs --shots shots.json --base <env-url> --out <shots-dir> [--login ...]`.
   - `--login` reads the QA test account from the credentials file that
     `DELIVERY.md` names and keeps the session in memory only. `--success-url`
     is required with `--login`. Anchor it to the signed-in path, as in `^https://[^/]+/admin/`,
     because an unanchored pattern also matches a sign-in URL that carries a
     `returnUrl`.
   - Never start interactive or third-party auth flows, such as OAuth consent,
     `vercel login` or `gh auth`, and never create accounts. They act as the
     operator. If the QA login fails, stop and report the failing step; the
     script saves `login-failure.png`.
   - Mask the client's personal data with each shot's `mask` selectors.

   A shot that lands on a different path than it requested fails as
   `redirected to …`, which is how a lost session shows. View every image.
   Re-shoot any image that shows a loading state, an error toast, a dev
   overlay, a logged-out page or empty data the requirement claims is
   populated. Done when the script exits 0, `manifest.json` lists every shot
   id and every image has been viewed.
7. **Write the markdown**
   `docs/sow/<sow-slug>/reports/<date>-<sow-slug>-round-<n>.md` to
   [references/report-contract.md](references/report-contract.md). Done when
   every DoD row and every round item appears exactly once and
   `python3 <skills-dir>/cas-sow-delivery-report/scripts/check-sow.py hygiene <report.md>`
   exits 0.
8. **Brief, render and critique.** Write the `cas-ui-craft` brief. The default
   hero is the one in report-contract.md. Render the HTML from
   [references/report-skeleton.html](references/report-skeleton.html) with
   the brand tokens as its header comment maps them, then make it a single
   file:
   `python3 <skills-dir>/cas-sow-delivery-report/scripts/inline-images.py <draft.html> <final.html>`.
   Run the strict visual QA from the `cas-ui-craft` critique rubric. This is a
   public surface, so distinctiveness, fit and hierarchy must each score at
   least 4. Done when visual QA prints PASS and the scored table is in the
   brief.
9. **Print and check the PDF.** Run
   `npm exec --yes --package=playwright -- node <skills-dir>/cas-sow-delivery-report/scripts/print-pdf.mjs <final.html> <out.pdf> [--author <report_author>]`,
   then `python3 <skills-dir>/cas-release-report/scripts/check-pdf.py <out.pdf> <artifacts_dir>/pdf`.
   Inspect every page PNG as `pdf.md` describes. Done when the receipt shows
   zero bounds findings and you have viewed every page.
10. **Commit the round** in one commit:
    - `GAP-CHECK.md` and `ROUNDS.md`;
    - the markdown, brief, HTML and PDF;
    - `visual-qa.md`;
    - the compressed `shots/*.jpg` with `manifest.json`.

    These shots are the next round's *befores*. Keep the full-resolution PNGs
    in the task artifacts directory. Done when `git show --stat HEAD` lists
    all of them.
11. **Draft the handoff** from [references/handoff.md](references/handoff.md)
    into `docs/sow/<sow-slug>/reports/<date>-<sow-slug>-round-<n>-handoff.md`.
    Fill every slot from `DELIVERY.md` and this round. Done when
    `grep -c '{' <handoff>` is 0. Post only after the operator's go, through
    `violet`. Its receipts are appended to the draft.

When the client's next bundle of feedback arrives, return to step 3 with
`n + 1`.

## Boundaries

`cas-release-report` owns version releases and `cas-release-notes` owns
merge announcements. This skill does not grant permission to post. The gap
check judges the SOW's own Outcome text, never a ticket list: the client
signed the SOW, not the backlog.
