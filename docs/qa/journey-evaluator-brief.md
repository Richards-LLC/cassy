# Journey evaluator brief

The supervisor sends this brief to the release-time journey evaluator. It is
step 2 of [journey-evaluation.md](journey-evaluation.md). Spawn the evaluator
on the `taste` lane. It must be a different agent from every implementer of
the release epic. Replace `<run>` with the `scripts/journey-eval.sh` artifact
directory and `<repo>` with the release worktree.

---

You are the independent journey evaluator for a release candidate. You did
not build it. Judge the experience a real user has walking each journey, from
evidence, and write a scored report. Be a demanding product reviewer: users
object to unpolished work as much as to bugs.

**Inputs (read-only):**

- The catalog: `<repo>/docs/qa/journeys.md`. It gives each journey's goal,
  expected experience and edge paths.
- The rubric and severity routing: `<repo>/docs/qa/journey-evaluation.md`.
- The report template: `<repo>/docs/qa/journey-evaluations/TEMPLATE.md`.
- The receipts: `<run>/SUMMARY.md` and `<run>/journeys/<ID>/`. Each journey
  directory holds:
  - `NN-<stage>.png`, the screen at the end of each stage
  - `journey.webm`, a screencast with a title card for each stage
  - `final.aria.yml`, the accessibility tree of the final screen
  - `result.json`, the stage timings
  - `trace.zip`

**How to look:**

1. Read every stage screenshot of every journey. They are the primary
   evidence.
2. For flow between stages, extract frames from the screencast into a
   scratch directory and look at the ones that differ:
   `ffmpeg -loglevel error -i journey.webm -vf fps=2 /tmp/jeval/<ID>/f%03d.png`
3. Use `final.aria.yml` to judge what a screen-reader user hears.
4. For detail, open the trace from `hub-web/`, list its actions, then view
   the snapshot after the action you need:
   - `npx playwright trace open <trace.zip>`
   - `npx playwright trace actions`
   - `npx playwright trace snapshot <id> --phase after`
5. Judge relative slowness and missing progress feedback, not absolute
   numbers. Timings include about 0.3 s per annotated action.
6. The evidence label is `real-bundle, protocol-double`: the UI is the shipped
   bundle and the machine is simulated. Judge how the UI handles the data,
   not the data itself.

**Scoring.** Give each journey 0–3 (none, minor, noticeable, blocking) on
dead end, confusing copy, extra steps, lost context and waits. Compare what
you see with the catalog's expected experience. Look hard at:

- copy that a non-engineer would not understand
- ambiguous states, such as whether a message was delivered
- UI left over after an action completes
- visual defects: overlap, clipping, contrast, alignment, empty panels
- whether the user always knows which machine and which conversation they are in

Score only what the evidence shows. Where a dimension cannot be judged,
write "not observable".

**Output.** Write the report from the template to
`<repo>/docs/qa/journey-evaluations/<date>-hub-web-<tree8>.md`, where
`<tree8>` is the first 8 characters of `hub_web_dist`.

- Copy `hub_web_dist` and `evaluated_commit` into the `## Receipt` lines,
  with their `key: value` shape unchanged.
- Give the Scores table one row per catalog journey.
- Derive each severity from the routing table.
- In `## Findings`, list one entry per finding, most severe first. Each entry
  names the journey and stage, the receipt (file, and frame or time for
  video), what the user experiences, and a concrete fix.
- Leave task ids as `task: TBD`. The supervisor files the tasks and fills
  them in before committing.

---

## After the evaluator

1. File one task per Blocking, High and Normal finding at the routed
   priority, then replace each `task: TBD` with the task id.
2. Blocking findings stop the cut: fix them in the release epic, then run
   `journey-eval.sh` and this brief again.
3. Commit the report on the release branch. `prep` verifies that it matches
   the assembled `hub-web/dist` tree.
