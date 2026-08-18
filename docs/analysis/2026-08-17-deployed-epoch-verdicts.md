# Deployed-binary symptom-recurrence verdicts

`cas history verdict` already owns the persistent `history_epochs` model and its conservative binary boundary; migration `m228_history_epochs_create_table` owns its storage. This report layer adds a re-runnable **batch seed contract**: a claimed fix carries its commit/build anchor and structured, lexical, and M2 semantic symptom signatures; the renderer produces one of `fixed`, `recurred`, or `insufficient-post-fix-data`, always naming the epoch boundary and observation exposure.

The distinction is deliberate. A merge or release tag establishes that a binary *could* contain a fix, but only an observed daemon epoch establishes that it was serving. Evidence in the interval between first fixed daemon and the last older/unknown daemon heartbeat is `mixed` and is never used to call a recurrence or a fix.

## Refreshing the v2.71.0 seed set after the normal v2.72 restart

> **Corrected 2026-08-18 (cas-2332).** The "no `history_epochs` table" measurement
> behind this deferral was taken against `~/.cas/cas.db`, which is the *global*
> store (entries/rules/sessions) and never holds epochs. The epoch-capable
> database is the project coordination DB — `<project>/.cas/cas.db` — which held
> 1360 `daemon_start` epochs at the time of writing. The seed run has since been
> executed against it; see
> [2026-08-18-v2.71-deployed-epoch-seed-run.md](2026-08-18-v2.71-deployed-epoch-seed-run.md)
> for the verdicts, and `scripts/seed_evidence_inputs.py` for the M1/M2 → M3
> input join this command assumes.

After the normal epoch-capable `cas serve` rebuild/restart and sufficient observations, capture timestamped evidence and evaluated M2 candidates, then run:

```bash
python3 docs/analysis/scripts/deployed_epoch_verdicts.py \
  --seeds docs/analysis/v2.71.0-fix-wave-seeds.json \
  --epochs-db "$(git rev-parse --show-toplevel)/.cas/cas.db" \
  --evidence /path/to/reviewed-post-restart-evidence.json \
  --semantic-evidence /path/to/m2-evaluated-evidence-scores.json \
  --output-json /path/to/v2.71-deployed-epoch-verdicts.json \
  --output-report docs/analysis/2026-08-17-v2.71-deployed-epoch-verdicts.md
```

`evidence` is a reviewable JSON list with `id`, RFC3339 `timestamp`, `source`, `text`, and optional `structured` labels. `semantic-evidence` maps each fix ID to M2's evaluated semantic channel, in either of two shapes: the original `{evidence_id: score}` map, or a declared evaluation `{"evaluated": true, "candidates_reviewed": n, "reviewer": "...", "scores": {...}}`. The second shape exists because the first cannot distinguish *reviewed every candidate and rejected them all* from *nobody looked* — both collapse to an empty map, and only the first may support a `fixed` verdict (cas-2332). A declared evaluation must name its reviewer and account for at least as many reviewed candidates as it reports positives; anything less is refused rather than trusted, because that guard is the last thing standing between an unobserved fix and a `fixed` verdict. The renderer calculates the structured and lexical baselines on every evidence unit too; semantic evidence does not replace them. An absent M2 evaluation prevents a `fixed` verdict, but direct clean-post recurrence evidence can still be reported. It never calls GitHub, Cassy task creation, or any network API. A `recurred` result contains a `draft-only-recurrence-proposal` in the JSON and report for human review, never an auto-filed issue/task.

## Fixture proof

`test_deployed_epoch_verdicts.py` runs the first-wave representative fixtures. They demonstrate all three report states, attach recurrence evidence cards and a human-review draft, and replay this session's merge-versus-epoch trap: the old daemon remains alive until 12:20Z, so an apparent symptom at 12:12Z is `mixed`, not post-fix evidence.

```bash
python3 -m unittest docs/analysis/scripts/test_deployed_epoch_verdicts.py
```

The checked-in `v2.71.0-fix-wave-seeds.json` contains the 20 concrete v2.71 wave anchors, including the session-end panic, stale-response, work-target, message-delivery, and epic-base fixes. The real v2.71 run remains a deployment-evidence follow-up, not a substituted fixture result.
