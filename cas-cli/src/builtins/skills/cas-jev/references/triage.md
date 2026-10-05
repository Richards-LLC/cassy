# Reviewed task-triage suggestions

Use this recipe for burn-down or issue sweeps. Every output needs independent
review before a task lifecycle action. The first production run (GH #1115)
rejected all six FIXED suggestions, four of five OBSOLETE suggestions and the
only DUPLICATE suggestion; it missed a narrower task covered by an umbrella.
The earlier evaluation had duplicate recall 0/4. The revised questions below
have offline routing regressions, not measured held-out model accuracy.

## Evidence and thresholds

Route with [triage.py](../scripts/triage.py), including at high confidence.
A suggested VALID, FIXED or OBSOLETE requires `evidence_complete: true`, a
Choice `confidence >= 0.9`, and at least one nonempty source citation supporting
that verdict at `target_sha`. FIXED also requires a complete candidate patch,
verified target integration, known dates, computed `predates_task: false` and
`resolution_noul >= 0.9`. OBSOLETE requires an explicit `retired_premise`.
A duplicate requires two nonempty scopes bound to a named pair and a separate
`scope_noul >= 0.9`; this pair signal can override a mistaken VALID suggestion.
These are conservative review floors, not calibrated accuracy guarantees.
Missing evidence, truncated input and unavailable calls route to UNCLEAR.

The helper checks structure, dates and thresholds. A reviewer must check that
citations actually establish the claim and that the paired task carries every
requirement. Never use a model's confidence as evidence of completeness.

## Recipe

1. Freeze `target_sha` and the task ids. Retrieve factual pre-triage descriptions,
   `created_at`, cited paths/symbols and notes. Keep human verdicts, terminal
   status and closure rationale out of model-visible state for an evaluation.
   Use `task action=list` to enumerate and `task action=show` for each id;
   save those records in the caller's evidence directory. Bulk full-task export
   is not provided by this recipe; completeness must be checked against the
   enumerated ids. Do not query the live store to work around this gap.
2. Retrieve candidate source and duplicate scopes. Use `search action=search
   doc_type=task query=<failure and requested behavior>` for duplicate candidates;
   then `task action=show` for each candidate. Include umbrella scopes, not just
   exact title matches. This is general retrieval, not a dedicated duplicate
   tool: retain retrieval misses for independent review.
   For commits use `git log --format='%H %cI %s' <target-sha> -- <cited-paths>`
   and `search action=history` for cited symbols. This is a manual cited-path
   recipe, not an automatic candidate-retrieval service. Verify membership with
   `git merge-base --is-ancestor <candidate-sha> <target-sha>`, read source patches
   with `git show`, and gather current code from `git show <target-sha>:<path>`.
   Record committer dates with `%cI`; retain pre-report candidates as context,
   but a pre-report commit cannot establish FIXED without newer repair evidence.
3. Build states with this schema; empty evidence stays empty:

   ```json
   {
     "record_id": "cas-example",
     "state": {
       "target_sha": "recorded-full-commit-sha",
       "task": {"id": "cas-example", "created_at": "2026-10-02T21:00:00Z", "description": "exact requested scope"},
       "evidence_complete": true,
       "current_code": [{"sha": "recorded-full-commit-sha", "path": "src/handler.rs", "line": 42, "snippet": "relevant source", "supports": "FIXED"}],
       "candidate_commits": [{"sha": "candidate-full-sha", "target_sha": "recorded-full-commit-sha", "committed_at": "2026-10-03T10:00:00Z", "integrated_on_target": true, "paths": ["src/handler.rs"], "patch": "relevant complete source patch", "resolution_noul": 0.95}],
       "similar_tasks": [],
       "observations": [{"observed_at": "2026-10-03T11:00:00Z", "source": "retained host check", "verified": true, "fact": "observed behavior"}]
     }
   }
   ```

   The helper computes `predates_task` from dates, overwriting a supplied flag;
   unknown or timezone-free dates cannot establish FIXED. For candidate calls,
   use `candidate_commit` instead of `candidate_commits`. For pair calls, use
   `task_id`, `other_id`, `task_scope` and `other_scope`; attach the returned
   Noul as `duplicate_candidate: {id, task_scope, other_scope, scope_noul}` in
   classification state, with that id/description also in `similar_tasks`.
   Record live facts in `observations`, not code snippets; live-only evidence
   remains UNCLEAR under this conservative code-based recipe.
4. Extract the three question maps from [triage-questions.json](triage-questions.json).
   Set `JEV_SKILL_DIR` to this skill and `JEV_EVIDENCE_DIR` to a new evidence
   directory; pin `jev.model` to the template's model.

   ```bash
   python3 - "$JEV_SKILL_DIR/references/triage-questions.json" "$JEV_EVIDENCE_DIR" <<'PY'
   import json, pathlib, sys
   q = json.loads(pathlib.Path(sys.argv[1]).read_text())["questions"]
   out = pathlib.Path(sys.argv[2]); out.mkdir(parents=True, exist_ok=True)
   for name, keys in (("candidate", ("candidate_resolves",)),
                      ("duplicate", ("duplicate_scope",)),
                      ("classification", ("verdict", "area", "severity"))):
       (out / (name + "-questions.json")).write_text(json.dumps({k: q[k] for k in keys}))
   PY
   ```

5. Run separate candidate and pair passes, then attach their answers and run
   classification. For each pass use the following commands, changing input,
   directory, mode and question map together (`candidate`, `duplicate` or
   `classification`). Every input row needs a unique `record_id` or `task.id`;
   candidate/pair ids must distinguish multiple comparisons of one task.

   ```bash
   python3 "$JEV_SKILL_DIR/scripts/triage.py" prepare --input "$JEV_EVIDENCE_DIR/tasks.jsonl" --directory "$JEV_EVIDENCE_DIR/classification" --tag burn-down-run-1 --mode classification
   for batch in "$JEV_EVIDENCE_DIR/classification"/batch-*.jsonl; do
     [ -f "$batch" ] || continue
     cas jev batch --input "$batch" --questions "@$JEV_EVIDENCE_DIR/classification-questions.json" --out "${batch%.jsonl}.answers.json" --advisory || exit 1
   done
   python3 "$JEV_SKILL_DIR/scripts/triage.py" stitch --directory "$JEV_EVIDENCE_DIR/classification" > "$JEV_EVIDENCE_DIR/suggestions.json"
   ```

   Preparation splits into batches of at most 50, caps each UTF-8 source hunk
   or candidate patch at 8 KiB, excludes generated/vendor paths, and refuses
   evaluation of incomplete or over-48-KiB states. Mark omitted evidence with
   `evidence_complete: false`; do not treat a capped prefix as negative evidence.
   Pair state must set `evidence_complete: true` when both scopes are complete.
   Stitch validates result counts, restores original order and retains excluded
   rows as unavailable/UNCLEAR rather than silently dropping them. Keep raw
   candidate/pair results for review; classification stitching applies the gate.
6. Review suggestions independently at the frozen revision. Preserve proposed
   and routed verdicts, cited code, missing evidence, duplicate id, raw answers
   and reviewer disposition. A model-only FIXED/OBSOLETE/DUPLICATE proposal
   cannot authorize cancellation. Re-evaluate revised questions on a blinded
   labelled cohort before claiming model accuracy or duplicate recall improved.

## CLI limits and correlation

`cas jev batch` still returns ordered arrays without record ids or request ids.
The helper's frozen manifest supplies `record_id` and caller `run_id`; a new
output directory is required to prevent mixing runs. `request_id` remains null
because the CLI response does not expose it. The transport id is in the shared
`.cas/jev-decisions.jsonl`, whose rows still lack run tags. CLI record-id echo,
transport-id echo and `--tag` are not implemented here. Retain per-run raw
answers and the manifest; do not attribute interleaved log rows by time windows
or input-token matching. Exact transport-log attribution needs client support.

The ten GH #1115 friction items map to: computed dates (step 3), explicit
retrieval and paired question (step 2/3), documented manual candidate lookup
(step 2), local record correlation and explicit missing request id (above),
local run tags and explicit shared-log limitation (above), byte caps and
source-only evidence (step 5), split/stitch (step 5), `observations` (step 3),
`store`/`cli` taxonomy (question map), and documented per-id export (step 1).
