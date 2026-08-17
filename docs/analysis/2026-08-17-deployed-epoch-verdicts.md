# Deployed-binary symptom-recurrence verdicts

`cas history verdict` already owns the persistent `history_epochs` model and its conservative binary boundary; migration `m228_history_epochs_create_table` owns its storage. This report layer adds a re-runnable **batch seed contract**: a claimed fix carries its commit/build anchor and structured, lexical, and M2 semantic symptom signatures; the renderer produces one of `fixed`, `recurred`, or `insufficient-post-fix-data`, always naming the epoch boundary and observation exposure.

The distinction is deliberate. A merge or release tag establishes that a binary *could* contain a fix, but only an observed daemon epoch establishes that it was serving. Evidence in the interval between first fixed daemon and the last older/unknown daemon heartbeat is `mixed` and is never used to call a recurrence or a fix.

## Refreshing the v2.71.0 seed set after the normal v2.72 restart

The live database available while this was built had no `history_epochs` table. Therefore this change does not publish invented v2.71 verdicts. After the normal epoch-capable `cas serve` rebuild/restart and sufficient observations, capture timestamped evidence and evaluated M2 candidates, then run:

```bash
python3 docs/analysis/scripts/deployed_epoch_verdicts.py \
  --seeds docs/analysis/v2.71.0-fix-wave-seeds.json \
  --epochs-db "$HOME/.cas/cas.db" \
  --evidence /path/to/reviewed-post-restart-evidence.json \
  --semantic-evidence /path/to/m2-evaluated-evidence-scores.json \
  --output-json /path/to/v2.71-deployed-epoch-verdicts.json \
  --output-report docs/analysis/2026-08-17-v2.71-deployed-epoch-verdicts.md
```

`evidence` is a reviewable JSON list with `id`, RFC3339 `timestamp`, `source`, `text`, and optional `structured` labels. `semantic-evidence` maps each fix ID to its stable evidence IDs and scores produced by M2's evaluated semantic channel. The renderer calculates the structured and lexical baselines on every evidence unit too; semantic evidence does not replace them. An absent M2 evaluation prevents a `fixed` verdict, but direct clean-post recurrence evidence can still be reported. It never calls GitHub, CAS task creation, or any network API. A `recurred` result contains a `draft-only-recurrence-proposal` in the JSON and report for human review, never an auto-filed issue/task.

## Fixture proof

`test_deployed_epoch_verdicts.py` runs the first-wave representative fixtures. They demonstrate all three report states, attach recurrence evidence cards and a human-review draft, and replay this session's merge-versus-epoch trap: the old daemon remains alive until 12:20Z, so an apparent symptom at 12:12Z is `mixed`, not post-fix evidence.

```bash
python3 -m unittest docs/analysis/scripts/test_deployed_epoch_verdicts.py
```

The checked-in `v2.71.0-fix-wave-seeds.json` contains the 20 concrete v2.71 wave anchors, including the session-end panic, stale-response, work-target, message-delivery, and epic-base fixes. The real v2.71 run remains a deployment-evidence follow-up, not a substituted fixture result.
