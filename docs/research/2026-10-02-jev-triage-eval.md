# Jev agreement with the v35 task triage

2026-10-02 · cas-9add · pinned model `jev-1.13.0`

**Decision: use Jev for human-reviewed suggestions.** Initial verdict agreement was **119/172 (69.19%)**; tuned wording reached **121/172 (70.35%)**. The 138 tasks excluded from item-level wording tuning fell from **98/138 (71.01%)** to **97/138 (70.29%)**. The tuned high-confidence bucket was **28/32 (87.50%)**, with **22/26 (84.62%)** on that held-out subset. These measurements do not support automatic task cancellation or a claim that wording tuning generalized.

The all-VALID majority baseline is 112/172 (65.12%). Tuned predictions proposing FIXED, OBSOLETE or DUPLICATE contain 14 human-VALID tasks. High confidence alone cannot compensate for missing current-state evidence.

## Cohort and blinding

The supervisor supplied `~/.cas/artifacts/jev-docs/eval-labels/v35-triage-labels.json`, the authoritative list of exactly 172 ids: VALID 112, FIXED 21, OBSOLETE 20, DUPLICATE 4, UNCLEAR 15. Later cas-f29b children were excluded. Cancellation reasons provide supporting evidence; there is no persisted rationale for human VALID/UNCLEAR labels and no independently hand-labelled area set. Thus agreement measures reproduction of these human verdicts, rather than objective correctness of every label.

Task data came from a read-only SQLite URI for the shared `.cas/cas.db`. The frozen Git input was `origin/main` at `2aa95f39323716583de2aae166f936679691c3ce`. The labels file SHA-256 is `094296a0ef11a0cc9c86ec7c324094a4249c434bc754ec4593beefa907315a62`. Both runs use the same frozen `states.json`; no task or cloud state was mutated.

Model-visible task fields are id, title, description (up to 9,000 characters) and the last 4,500 characters of filtered pre-triage notes. Status, priority, close reason, assignee, the structured labels field, human verdict and reference duplicate-target field are excluded. Notes at or after 2026-10-02 12:50, terminal lifecycle records and sync/relocation records are removed. Existing pre-triage factual resolution notes remain legitimate evidence. Similar-task texts contain no labels or status.

Before inference, SHA-256 ordering of task ids selected 34 calibration tasks and 138 held-out tasks. Item-level tuning inspected only the 34 calibration examples. Overall and held-out aggregate run1 scores were visible before tuning. This is a single split on one historical corpus, not a fully unseen prospective validation set. There was one wording revision and exactly two inference runs, with no threshold sweep.

## Questions and execution

The exact initial questions are in [jev-triage-questions-run1.json](jev-triage-questions-run1.json); the final reusable set is [jev-triage-questions.json](jev-triage-questions.json). Machine-readable measurements, confusion matrices and all disagreements are in [jev-triage-eval-results.json](jev-triage-eval-results.json).

Code resolves cited paths against the frozen main tree, including unique basenames; it searches the last 200 path commits and full-main subjects mentioning task ids. It ranks by id match, then token overlap, retaining at most four commits per task. It retrieves three similar task titles/descriptions and excerpts around two token-matching locations in each of at most three current files. Each candidate carries a subject and the first 10,000 characters of its patch. The model does not search Git or infer commit reachability.

Each candidate is judged in a separate request with the `candidate_resolves` Noul template against only `task` and `candidate_commit`. The classification request then gets the task, similar tasks, current source excerpts, candidate patches and their resolution Nouls, with `verdict` Choice, `area` Choice and `severity` Score. These are 544 candidate requests and 172 classification requests per run. Nouls are fallible evidence; there is no automatic conversion from a Noul threshold to a verdict.

The wording revision makes obsolete pre-merge reviews explicit, distinguishes an umbrella duplicate from a shared topic, describes live-host/intermittent uncertainty, requires shown patch evidence for resolution, and distinguishes frontend ownership from the QA checker. Candidate lists, snippets and task states remain fixed. Severity wording is unchanged.

## Agreement

| Metric | Initial | Tuned |
|---|---:|---:|
| Overall | 119/172 (69.19%) | 121/172 (70.35%) |
| Calibration subset | 21/34 (61.76%) | 24/34 (70.59%) |
| Held-out subset | 98/138 (71.01%) | 97/138 (70.29%) |
| Area against explicit labels | 35/40 (87.50%) | 32/40 (80.00%) |

Verdict buckets use the API-returned Choice `confidence`, not the largest option probability. Exactly 0.9 enters the upper bucket; exactly 0.5 enters the middle bucket.

| Confidence | Initial n | Initial correct | Initial accuracy | Tuned n | Tuned correct | Tuned accuracy |
|---|---:|---:|---:|---:|---:|---:|
| >=0.9 | 75 | 60 | 80.00% | 32 | 28 | 87.50% |
| 0.5-0.9 | 79 | 51 | 64.56% | 95 | 71 | 74.74% |
| <0.5 | 18 | 8 | 44.44% | 45 | 22 | 48.89% |

The tuned upper bucket covers only 32/172 (18.60%) of tasks and includes four disagreements, all in the held-out subset: cas-563a, cas-939a, cas-0bad and cas-fa64. Confidence is a useful ranking signal here; its nominal level does not establish production calibration.

Rows below are human labels; columns are Jev choices. Every matrix totals 172.

### Initial confusion matrix

| Human / Jev | VALID | FIXED | OBSOLETE | DUPLICATE | UNCLEAR |
|---|---:|---:|---:|---:|---:|
| VALID | 107 | 3 | 0 | 1 | 1 |
| FIXED | 9 | 12 | 0 | 0 | 0 |
| OBSOLETE | 17 | 1 | 0 | 2 | 0 |
| DUPLICATE | 4 | 0 | 0 | 0 | 0 |
| UNCLEAR | 15 | 0 | 0 | 0 | 0 |

### Tuned confusion matrix

| Human / Jev | VALID | FIXED | OBSOLETE | DUPLICATE | UNCLEAR |
|---|---:|---:|---:|---:|---:|
| VALID | 97 | 9 | 4 | 1 | 1 |
| FIXED | 7 | 13 | 1 | 0 | 0 |
| OBSOLETE | 10 | 1 | 9 | 0 | 0 |
| DUPLICATE | 3 | 0 | 0 | 0 | 1 |
| UNCLEAR | 12 | 0 | 1 | 0 | 2 |

Tuned class recall is VALID 97/112 (86.61%), FIXED 13/21 (61.90%), OBSOLETE 9/20 (45.00%), DUPLICATE 0/4 (0%), and UNCLEAR 2/15 (13.33%). Obsolete-review wording corrected calibration examples, while duplicate and uncertainty decisions remain weak.

## Area and severity

The label-derived area reference covers 40/172 (23.26%) tasks. Exact label aliases map hub-web/commander, cloud, ci, release, qa, factory, close-gate, hooks and violet/slack to the question options. Multiple conflicting recognized labels and tasks without recognized labels are excluded. This is a limited proxy derived from existing labels; 132 tasks have no area truth. The script records each original labels array and selected proxy area in `gold.json`.

| Predicted area | Initial n | Initial mean confidence | Tuned n | Tuned mean confidence |
|---|---:|---:|---:|---:|
| hub-web | 38 | 0.936 | 37 | 0.921 |
| cloud | 8 | 0.871 | 4 | 0.840 |
| ci | 8 | 0.850 | 11 | 0.702 |
| release | 11 | 0.798 | 6 | 0.882 |
| qa | 20 | 0.788 | 16 | 0.836 |
| factory | 28 | 0.866 | 24 | 0.876 |
| close-gate | 20 | 0.794 | 15 | 0.777 |
| hooks | 10 | 0.842 | 8 | 0.770 |
| memory-search | 8 | 0.875 | 3 | 0.860 |
| slack | 3 | 0.870 | 3 | 0.653 |
| other | 18 | 0.678 | 45 | 0.732 |

Severity is a four-level Score: 0 cosmetic/optional; 1 minor/workaround; 2 workflow failure; 3 outage/data loss/credential exposure/isolation breach. Initial mean score 1.696 and tuned mean 1.698; modal-level counts are respectively 5/45/115/7 and 5/45/114/8. There are no human severity labels, so severity accuracy is unmeasured. The score expectation is not an exact impact magnitude and is not equated to task priority.

## Cost and latency

The [model price](https://docs.typesafe.ai/models) verified on 2026-10-02 is $0.042 per million input tokens, with free output tokens. Cost below is token-accounted API cost, excluding local preparation, taxes and any invoice adjustments. All responses report `jev-1.13.0`. The [HTTP API](https://docs.typesafe.ai/api) is called directly with a credential read inside the script from the authorized 0600 file; request headers and API error bodies are never logged.

| Measurement | Initial | Tuned |
|---|---:|---:|
| Successful requests / attempts | 716/716 | 716/716 |
| Failed requests / retries | 0 / 0 | 0 / 0 |
| Input tokens | 3,261,682 | 3,327,365 |
| Output tokens (free) | 41,496 | 41,517 |
| Estimated API USD | $0.13699064 | $0.13974933 |
| Median request latency | 193.3 ms | 180.1 ms |
| p95 request latency | 287.8 ms | 276.6 ms |
| Run wall time, 8 workers | 19.51 s | 18.46 s |
| Sum of request durations | 148.62 s | 142.25 s |

Total estimated API cost is **$0.27673997**, over 1,432 requests. Latency is client-observed HTTP duration including connection setup, network and decoding; it is not isolated model compute time. Wall time excludes database/Git preparation and report generation.

## Annotated disagreements and corrected examples

The following diagnoses come from the frozen task state, raw responses and human supporting evidence. They are analysis of input or decision failures; they are not generated explanations from Jev, which returns decisions only.

### cas-746a

Human **FIXED**; initial **VALID (0.47)**; tuned **VALID (0.33)**.

Input truncation. Human FIXED cites `1d78a4898`, `scripts/cas-install.sh:321-325`. That commit is in the four-candidate shortlist, but the 10,000-character full patch excerpt contains no `tmp_dir`: earlier documentation changes consumed the budget. Candidate Noul was 0.02 in run1 and 0.03 in run2. A negative judgment on the incomplete excerpt cannot establish absence of the real fix. Prioritize cited-file hunks before truncation.

### cas-05dd

Human **DUPLICATE**; initial **VALID (0.92)**; tuned **VALID (0.78)**.

Duplicate retrieval miss. Human DUPLICATE folds the missing `cloud.team_only` registry key into umbrella cas-8095. The three title-similar tasks omit cas-8095, although the narrow task is part of its fix. The `aa07043c5` opt-in candidate alone does not register the key. Retrieve by exact symbols and descriptions, and present the umbrella scope; wording alone cannot choose an absent target.

### cas-72163

Human **DUPLICATE**; initial **VALID (0.62)**; tuned **VALID (0.58)**.

Duplicate decision failure with available evidence. Human DUPLICATE names cas-7cb3. That task is first in `similar_tasks`; its title says “Copy covers Details,” its description explicitly calls out F40 and refers to cas-72163. Jev still chooses VALID. This is stronger evidence of a decision failure than cas-05dd, whose target was absent.

### cas-563a

Human **OBSOLETE**; initial **VALID (0.98)**; tuned **VALID (0.98)**.

High-confidence current-state failure. Human OBSOLETE cites identity-only verification authority at `verification_tools.rs:130-175`, with the old magic 300/“5min” removed. Jev chooses VALID at 0.98 in both runs. Its shortlist instead emphasizes earlier verification error-message work; sparse token-selected current snippets do not prove the old premise is gone. A symbol-presence/absence scan belongs in code before asking the literal relevance question.

### cas-939a

Human **OBSOLETE**; initial **VALID (0.95)**; tuned **VALID (0.94)**.

High-confidence retired-feature miss. Human OBSOLETE cites `849a49f38` removing `run_code_review_gate`, `format_block_message` and `evaluate_gate`. That SHA is absent from the shortlist, which emphasizes older envelope/forgery changes. VALID confidence stays 0.95→0.94. Retrieve removed-symbol history and provide present-day symbol results before assuming polish work remains.

### cas-0bad

Human **FIXED**; initial **VALID (0.97)**; tuned **VALID (0.96)**.

High-confidence fix miss. Human FIXED cites `6b3991ba3`, with JSON run-report authority at `qa_evidence.rs:560-562`. Retrieval returns four unrelated hub-web rebuild commits instead. Old task notes explicitly describing an unmerged bare-PASS workaround push the decision toward VALID (0.97→0.96). Current relevant gate code and its actual change history should outrank incidental frontend rebuilds.

### cas-4e45

Human **UNCLEAR**; initial **VALID (0.98)**; tuned **VALID (0.59)**.

Unavailable external facts / label rationale. Human UNCLEAR has no persisted explanation. The task reports a repeatable Violet thread-expansion failure in another repository; origin/main gives no candidate commits. Initial VALID confidence 0.98 falls to 0.59 after uncertainty wording, but the verdict stays VALID. The report does not assert the human label is objectively right; the frozen Cassy state cannot settle current Violet behavior.

### cas-b8fc

Human **OBSOLETE**; initial **FIXED (0.62)**; tuned **FIXED (0.38)**.

Historical completion versus subsequent reversal. Human OBSOLETE cites `bf82cba1c` and current `lane-registry.toml:111,162-165` restoring the prior taste lane. Four id-matching commits all describe the older migration; Jev picks FIXED (0.62→0.38), and the earlier corrective commit Noul rises 0.75→0.81. The main-reachable historical implementation does not establish the current policy. Rank the newest relevant reversal and give exact current policy fields.

### cas-1502

Human **VALID**; initial **VALID (0.93)**; tuned **UNCLEAR (0.40)**.

Tuning regression. Human VALID reports rule writes failing under SQLite contention. Initial VALID at 0.93 becomes UNCLEAR at 0.40. The new intermittent/live-state rubric also captures a concrete code defect with a known requested change. This tradeoff is why calibration improvements cannot stand in for held-out gains.

### cas-844b

Human **OBSOLETE**; initial **VALID (0.72)**; tuned **OBSOLETE (0.91)**.

Corrected calibration example. Human OBSOLETE is a pre-merge review of cas-219d already shipped in `567c21c`. Initial VALID at 0.72 becomes OBSOLETE at 0.91 after the explicit one-off pre-merge-review criterion. This wording works for the shown example, but the aggregate held-out score did not improve.

## Limits and next decision

Shortlist coverage is limited: at least one human-cited SHA was retrieved for 15/37 tasks with cited hashes (12/18 cited-hash FIXED tasks). This counts exact prefix matches to cited commits; it does not prove full semantic recall, and merge equivalents can be missed. Only 2/4 named duplicate targets occur in the three-task shortlist. Twenty-one tasks have no candidate commits. Patch truncation, subject-only ranking, substring id matching and sparse source excerpts introduce input failure before inference. These are observed properties of this evaluated retrieval recipe, not measurements of Jev with complete evidence.

Candidate Nouls lack comprehensive candidate-level human labels; no Noul accuracy or calibrated acceptance threshold is claimed. Present-day file snippets cannot establish an absent symbol or host condition unless code supplies that fact. Pre-triage historical success notes can also outweigh a later reversal if retrieval fails to include it. Human VALID/UNCLEAR labels lack the evidence needed to distinguish model errors from unavailable human context.

Keep the final question set as a reviewed suggestion recipe. Before another evaluation, improve retrieval with exact task-id boundaries, cited-symbol history, explicit newest-state facts, cited-file patch excerpts, and description/symbol duplicate matching. Route unresolved external runtime facts to a real check. Validate any automatic action threshold on a new labelled batch; tuning this batch again would compound overfitting. No task lifecycle action or production threshold is enabled by this spike.

## Reproduction and durable evidence

Runner: [jev_triage_eval.py](scripts/jev_triage_eval.py). Cross-check: [verify_jev_triage_eval.py](scripts/verify_jev_triage_eval.py). Python standard library only; run from the repository root. `prepare` reads the database, then freezes state. Reuse the frozen state for comparison; a fresh database extraction is a different experiment. The key is read in-process and must be a single nonempty line with mode 0600 or stricter. The runner resumes matching question sets; use a new run name for a changed question set.

Artifacts: `/Users/pippenz/.cas/artifacts/cas-src-ec436edb9fa83e1c1bea3ce9f62e009b4ec7a1295a20c57e7bc77fc869972300/cas-9add/`.

| Evidence | File |
|---|---|
| Frozen inputs and reference labels | `states.json`, `gold.json`, `manifest.json` |
| Exact questions, 172 raw records, accounting | `run1/` and `run2/` |
| Wording decision before second inference | `tuning-notes.md` |
| Recomputed summaries and token/latency totals | `run1/summary.json`, `run2/summary.json` |
| Cross-check receipt | `verification.log` |
| This report | `2026-10-02-jev-triage-eval.md` |

```bash
# Point ARTIFACT_DIR to the artifact path above.
python3 docs/research/scripts/jev_triage_eval.py prepare --db /path/to/.cas/cas.db --labels /path/to/v35-triage-labels.json --out "$ARTIFACT_DIR"
python3 docs/research/scripts/jev_triage_eval.py run --out "$ARTIFACT_DIR" --questions docs/research/jev-triage-questions-run1.json --name run1
python3 docs/research/scripts/jev_triage_eval.py run --out "$ARTIFACT_DIR" --questions docs/research/jev-triage-questions.json --name run2
python3 docs/research/scripts/jev_triage_eval.py summary --out "$ARTIFACT_DIR" --name run1
python3 docs/research/scripts/jev_triage_eval.py summary --out "$ARTIFACT_DIR" --name run2
python3 docs/research/scripts/verify_jev_triage_eval.py "$ARTIFACT_DIR"
```
