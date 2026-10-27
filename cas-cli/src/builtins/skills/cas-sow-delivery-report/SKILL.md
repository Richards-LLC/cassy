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
- `cas deliverable finalize`: brand fill from `DESIGN.md`, single-file
  inlining within the customer budget, client hygiene, print and the PDF
  check, in one command.

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
5. **Write and commit the shot list** `docs/qa/shots.json`, an array of
   `{ "id": "R1", "path": "/admin/dashboard/", "viewport": [1280, 800],
   "ready": "main", "locator": "main", "mask": [".email"] }` objects:
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
6. **Capture through the deliverables core.** Configure `[qa.capture]` in
   `.cas/config.toml`: `serve` with a `{port}` placeholder for local revisions,
   or `base_url` for a deployed environment; `shots` names the committed list.
   Set `credentials_env` to an environment-variable name whose value is the
   readable QA credential-file path (`email:` and `password:` lines). Set
   `login_path` and an anchored `success_url`, for example
   `^https://[^/]+/admin/`, plus the project's form selectors. Never put the
   account secret in config or a report. Keep the session in memory.
   - Run `cas deliverable capture --task <task> --target <branch>` for a
     merge-base/head pair from immutable checkouts in one bounded QA run.
   - For production/staging, configure the environment and run
     `cas deliverable capture --task <task> --side before`, then configure the
     delivered environment and run the same command with `--side after`.
     Both runs use the same shot IDs, framing and masks. Preserve the receipt
     source; never label a deployed screenshot as a Git build.
   - Without a catalog, the core runs affected journeys at both revisions and
     pairs their final-stage screenshots only when both runs pass.
   - Mask personal data with `qa.capture.mask` or each shot's `mask`. A missing
     requested mask fails capture. Auth failures produce a finding without an
     unmasked failure screenshot. Follow the normal PM handoff for unavailable
     evidence; never create an account or start third-party consent.

   Run `cas deliverable pairs --task <task>` to select verified complete pairs
   from `<task-artifacts>/qa/pairs/pairs.json`. An unpaired capture is excluded.
   View every image and re-shoot loading states, errors, logged-out screens or
   empty data. Done when capture exits 0, every required shot has both sides
   with matching framing/masks, and every image has been viewed.
7. **Write the markdown**
   `docs/sow/<sow-slug>/reports/<date>-<sow-slug>-round-<n>.md` to
   [references/report-contract.md](references/report-contract.md). Done when
   every DoD row and every round item appears exactly once.
8. **Brief, render and finalize.** Write the `cas-ui-craft` brief. The default
   hero is the one in report-contract.md. Fill a copy of
   [references/report-skeleton.html](references/report-skeleton.html) with the
   markdown's content, keeping its brand marker, then run
   `cas deliverable finalize <draft.html> --audience customer --out <final.html> --pdf a4,letter --author <report_author> --check-dir <artifacts_dir>/pdf`.
   It brands the page from the project's `DESIGN.md`, inlines every shot
   within the customer budget (it names any pair it had to leave out), checks
   the visible text for internal vocabulary, then prints A4 and Letter and
   checks both. Exit 1 names each finding; fix the source, not the output.
   Done when it exits 0.
9. **Critique and view every page.** Run the strict visual QA from the
   `cas-ui-craft` critique rubric on the final HTML. This is a public
   surface, so distinctiveness, fit and hierarchy must each score at least 4.
   Inspect every page PNG under `<artifacts_dir>/pdf` as
   `cas-release-report/references/pdf.md` describes. Done when visual QA
   prints PASS, the scored table is in the brief and you have viewed every
   page.
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
