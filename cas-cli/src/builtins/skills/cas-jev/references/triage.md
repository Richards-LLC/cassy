# Reviewed task-triage suggestions

Use this branch for a burn-down or a `cas-github-issues` sweep. Keep Jev outputs
as suggestions for review, including at high confidence; this recipe grants
no automatic cancellation, closure, duplicate merge or priority change.

The [172-task agreement evaluation](https://github.com/Richards-LLC/cassy/blob/cf89eb9ad7e0cb47012a929e68ac5e2683e07650/docs/research/2026-10-02-jev-triage-eval.md)
measured about 70% verdict agreement (tuned 121/172 = 70.35%; held-out
97/138 = 70.29%). Its tuned `confidence >= 0.9` bucket agreed on 28/32 = 87.50%
overall and 22/26 = 84.62% held-out: roughly 84–88%, with few examples and
four disagreements. High confidence cannot repair missing retrieval. Duplicate
recall was 0/4; no human labels measured severity accuracy. Wording gains on
calibration did not generalize to held-out tasks.

## Recipe

1. Freeze the evidence. Record the target branch/SHA and task ids. Retrieve
   title, description and factual pre-triage notes, exact cited symbols/paths,
   relevant current code, candidate implementation patches and potential
   duplicate scopes. Keep any human verdict, terminal status and closure
   rationale out of model-visible state during evaluation. Done when a reviewer
   can locate each cited hunk or missing fact at the recorded revision.
2. Prepare the shipped questions. Use
   [triage-questions.json](triage-questions.json), a copy of the final evaluated
   `docs/research/jev-triage-questions.json`. Preserve its wording and criteria
   for comparable suggestions. For another target branch, adapt both literal
   main statements and record the target before either pass. Resolve paths
   relative to this skill directory;
   set `JEV_SKILL_DIR` to that directory and `JEV_EVIDENCE_DIR` to the caller's
   evidence directory. Extract question maps for the two distinct passes:

   ```bash
   python3 - "$JEV_SKILL_DIR/references/triage-questions.json" "$JEV_EVIDENCE_DIR" <<'PY'
   import json, pathlib, sys
   questions = json.loads(pathlib.Path(sys.argv[1]).read_text())["questions"]
   out = pathlib.Path(sys.argv[2]); out.mkdir(parents=True, exist_ok=True)
   (out / "candidate-questions.json").write_text(json.dumps({"candidate_resolves": questions["candidate_resolves"]}))
   (out / "classification-questions.json").write_text(json.dumps({k: questions[k] for k in ("verdict", "area", "severity")}))
   PY
   ```

   Keep `jev.model` pinned to the template's `jev-1.13.0`. Done when candidate
   questions contain only `candidate_resolves` and classification questions
   contain `verdict`, `area` and `severity`.
3. Judge candidate patches separately. For each verified candidate already
   on the recorded branch, prepare a state with `task` and `candidate_commit`
   (SHA, subject and relevant patch). Prefer cited-file hunks before truncation;
   mark omitted content. Write 1–50 such objects per candidate JSONL batch:

   ```bash
   cas jev batch --input "$JEV_EVIDENCE_DIR/candidates.jsonl" --questions "@$JEV_EVIDENCE_DIR/candidate-questions.json" --out "$JEV_EVIDENCE_DIR/candidate-answers.json" --advisory
   ```

   Done when each candidate's `candidate_resolves.noul` is attached to its SHA,
   or marked unavailable. Treat a Noul as fallible evidence, not a fix verdict;
   a low value on a truncated patch does not prove the absence of a fix.
4. Request the three classifications. Build each task state with `task`,
   `similar_tasks`, `candidate_commits` (including `resolution_noul` signals)
   and `current_main_snippets`. Send structured state through `jev action=ask`,
   or write a classification JSONL batch:

   ```bash
   cas jev batch --input "$JEV_EVIDENCE_DIR/tasks.jsonl" --questions "@$JEV_EVIDENCE_DIR/classification-questions.json" --out "$JEV_EVIDENCE_DIR/suggestions.json" --advisory
   ```

   Done when each task has a suggested VALID/FIXED/OBSOLETE/DUPLICATE/UNCLEAR,
   area and impact score, or an explicit unavailable result. Use returned
   verdict confidence to order review; treat the impact score as a rubric
   suggestion, not an automatic mapping to Cassy priority.
5. Review and record the result. Independently verify the claimed fix on
   the target revision, the retired premise or the duplicate's matching scope.
   For UNCLEAR, name the missing live check or ambiguous requirement. Apply
   existing sweep/task authorization rules to any lifecycle action. Record
   the proposed verdict, probabilities/confidence, evidence revision, reviewed
   disposition and relevant decision-log row. Done when every suggestion is
   accepted with independent evidence, rejected, or left for human review.

For question changes, freeze a labelled cohort, separate wording calibration
from held-out cases and compare both on identical retrieved evidence. Compute
agreement, counts and costs in code. Keep unavailable results and retrieval
misses visible; do not replace them with guessed model decisions.
