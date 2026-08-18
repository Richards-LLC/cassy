# Factory failure-mode frequency audit v1

- **Date:** 2026-08-11
- **Audience:** Cassy factory practitioners
- **Report contract:** Metrics / mining analysis
- **Window:** 2026-07-28 00:00 EDT through 2026-08-11 13:55:35 EDT
- **Confidence:** High for the named evidence and counts in the adjudicated manifest; medium for cross-model comparison because task mix and model assignment were not randomized.

## 1. Headline finding

The two-week corpus contains **24 observed failure events in 21 model/session-class episodes across 1,135 factory sessions**. Three classes carry 14 of the 24 events (58.3%): workspace-contract denials (6), missed surfaces (4), and crossed-message merge races (4). The broad lexical pass initially surfaced 586 candidates; structural evidence framing reduced that to 31, and manual adjudication accepted 21 episodes. In total, 565 broad candidates (96.4%) did not become incidents, which is why this report treats the final counts as a high-precision lower bound rather than turning prompt echoes into telemetry.

## 2. Overview

| Metric | Current | Baseline | Delta | % delta | Sample |
| --- | ---: | ---: | ---: | ---: | ---: |
| Adjudicated observed events | 24 | 586 lexical candidates | -562 | -95.9% | 1,135 sessions |
| Events in top three classes | 14 | 10 in the other six classes | +4 | +40.0% | 24 events |
| Claude Fable 5 affected-episode rate | 6.98% | 1.85% corpus rate | +5.13 points | +277.1% | 86 sessions |
| GPT-5.6 Terra affected-episode rate | 4.62% | 1.85% corpus rate | +2.77 points | +149.5% | 195 sessions |
| Claude Opus 5 affected-episode rate | 1.45% | 1.85% corpus rate | -0.40 points | -21.4% | 275 sessions |
| GPT-5.6 Sol affected-episode rate | 0.43% | 1.85% corpus rate | -1.42 points | -76.5% | 460 sessions |

“Affected-episode rate” is model/session-class episodes divided by sessions for that model. It is not a model-quality score: assignments, repositories, supervisors, and guard exposure differ materially.

## 3. Method

### 3.1 Corpus and window

The inclusive start is `2026-07-28T04:00:00Z`; the exclusive end is `2026-08-11T17:55:35Z`, immediately before the audit worker spawned. The final extraction found **437 Claude factory sessions** and **698 Codex factory sessions** with an in-window record, for **1,135 total**. File discovery used the transcript record timestamp as the authority; mtime only skipped files that could not contain an in-window append. The frozen session summary records these denominators: a verification rerun found three previously dormant sessions after their append-only files resumed (2 Sol, 1 Terra), but no additional evidence candidate. Exact historical reproduction therefore uses the checked-in summary; a live-source rerun may discover another late append whose record timestamp falls inside the fixed window.

Primary sources:

- `~/.claude/projects/**/*.jsonl` and `~/.claude-alt/projects/**/*.jsonl`;
- `~/.codex/sessions/**/*.jsonl`; and
- a read-only copy of `/home/pippenz/Petrastella/cas-src/.cas/cas.db` plus WAL/SHM, whose copied snapshot returned `PRAGMA integrity_check = ok`.

The source inventory before factory filtering was 2,628 Claude files / 1.25 GB and 1,123 Codex files / 2.32 GB modified in the window. Grok had 90 files / 14.7 MB, but v1 excludes Grok from the denominator because the assigned source contract named Claude project JSONL, Codex session JSONL, and factory notes; Grok’s post-window schema and coverage are not comparable. A missing Grok row therefore means **not measured**, not zero failures.

### 3.2 Unit, extraction, and adjudication

The frequency unit is a unique **model × harness × session/work-episode × failure class**. Repeated evidence in one session/class is one episode; the manifest separately records raw occurrences. That is why the three zero-test invocations in one Claude Opus 5 session appear as `1 episode / 3 events`.

`docs/analysis/scripts/mine_failure_modes.py` streams JSONL, inherits model from Claude assistant metadata or Codex turn context, excludes system/developer rows and Cassy task-show/sibling-note dumps, and emits one candidate pointer per source/session/class. Two evidence frames are accepted:

1. a unique runtime guard banner in tool output for unscoped tests or workspace-contract denial; or
2. a concrete supervisor correction, close rejection, blocker/discovery note, or task-note incident carrying the actual missing action or damage.

Assistant discussion, task descriptions that merely name a category, preventive guidance, test fixtures, and release-note prose are rejected. The checked-in `2026-08-11-failure-mode-incidents.csv` is the adjudicated manifest. Counts below are pivots of that file, not of raw keyword hits.

Reproduction:

```bash
tmpdir=$(mktemp -d)
cp /home/pippenz/Petrastella/cas-src/.cas/cas.db "$tmpdir/snap.db"
cp /home/pippenz/Petrastella/cas-src/.cas/cas.db-wal "$tmpdir/snap.db-wal"
cp /home/pippenz/Petrastella/cas-src/.cas/cas.db-shm "$tmpdir/snap.db-shm"
sqlite3 "$tmpdir/snap.db" 'PRAGMA integrity_check;'

python3 docs/analysis/scripts/mine_failure_modes.py \
  --since 2026-07-28T04:00:00Z \
  --until 2026-08-11T17:55:35Z \
  --claude-root ~/.claude/projects \
  --claude-root ~/.claude-alt/projects \
  --codex-root ~/.codex/sessions \
  --task-db "$tmpdir/snap.db" \
  --output "$tmpdir/candidates.csv" \
  --summary "$tmpdir/summary.json"
```

Human rerun step: inspect every candidate against the evidence-frame rule, aggregate repeated occurrences in the same session/class, and write the accepted rows to the manifest. V1 intentionally does not ask a model to auto-label its own failures.

### 3.3 Category definitions

| Failure class | Count when… | Exclude when… |
| --- | --- | --- |
| Unscoped test guard | a run is unscoped, resolves no package/filter, or executes zero tests while treated as green | a fixture or guidance merely quotes the guard |
| Workspace-contract denial | the live hook banner rejects a write target | source/tests merely contain the banner text |
| Wrong process/environment | the worker damages or disconnects its own task environment or targets the wrong process/store | it identifies and preserves unrelated processes |
| Scope drift | work actually leaves the task body, or a false drift signal blocks correct work | a supervisor says “keep X out of scope” and the worker complies |
| Partial delivery | a task closes or requests merge with a named acceptance slice absent | intentionally phased work remains open and named |
| Missed surface | a sibling mirror, migration pin, generated config, behavior-contract test, or reverse state is omitted | all applicable surfaces are checked with evidence |
| Crossed-message merge race | a merge/amendment and worker action cross, forcing a redundant wait, close, or correction | an ordinary MERGE REQUIRED handoff completes in order |
| Premature done claim | close/done is claimed before surfaced amendments or fresh proof are incorporated | preventive “verify before close” prose |
| Draft/format violation | an output violates its required message/document structure and needs reinterpretation or rework | an ordinary formatter task is assigned |

## 4. Results

### 4.1 Model × harness × failure class

Cells are **episodes / raw events**. A dash is an observed zero inside the measured corpus; `n` is the number of factory sessions for that row.

| Harness | Model | n | Unscoped test | Workspace denial | Wrong process/env | Scope drift | Partial delivery | Missed surface | Crossed race | Premature done | Draft/format |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Claude | `<synthetic>` | 19 | — | — | — | — | — | — | — | — | — |
| Claude | `claude-fable-5` | 86 | — | **6 / 6** | — | — | — | — | — | — | — |
| Claude | `claude-haiku-4-5-20251001` | 14 | — | — | — | — | — | — | — | — | — |
| Claude | `claude-opus-4-5-20251101` | 8 | — | — | — | — | — | — | — | — | — |
| Claude | `claude-opus-4-8` | 1 | — | — | — | — | — | — | — | — | — |
| Claude | `claude-opus-5` | 275 | **1 / 3** | — | — | **1 / 1** | — | — | — | **1 / 1** | **1 / 2** |
| Claude | `claude-sonnet-5` | 24 | — | — | — | — | — | — | — | — | — |
| Claude | `unknown` | 10 | — | — | — | — | — | — | — | — | — |
| Codex | `claude-opus-4-5` | 2 | — | — | — | — | — | — | — | — | — |
| Codex | `gpt-5.5` | 39 | — | — | — | — | — | — | — | — | — |
| Codex | `gpt-5.6-luna` | 2 | — | — | — | — | — | — | — | — | — |
| Codex | `gpt-5.6-sol` | 460 | — | — | — | — | — | — | **2 / 2** | — | — |
| Codex | `gpt-5.6-terra` | 195 | — | — | **1 / 1** | — | **2 / 2** | **4 / 4** | **2 / 2** | — | — |
| **Total** | **all measured** | **1,135** | **1 / 3** | **6 / 6** | **1 / 1** | **1 / 1** | **2 / 2** | **4 / 4** | **4 / 4** | **1 / 1** | **1 / 2** |

### 4.2 Frequency by class

| Rank | Failure class | Episodes | Raw events | Events / 1,135 sessions | Named evidence pointer |
| ---: | --- | ---: | ---: | ---: | --- |
| 1 | Workspace-contract denial | 6 | 6 | 0.53% | `~/.claude/.../54bc7a03-….jsonl:29` (all six exact pointers are in the manifest) |
| 2= | Missed surface | 4 | 4 | 0.35% | `task:cas-641f@description` → `cas-8921`, `cas-96f9`, `cas-2327`, `cas-bc13` |
| 2= | Crossed-message merge race | 4 | 4 | 0.35% | `~/.codex/sessions/2026/08/11/rollout-…019ff0a6….jsonl:441`; `task:cas-1e7e@2026-08-11T14:58:05Z` |
| 4 | Unscoped test guard | 1 | 3 | 0.26% | `~/.claude-alt/.../steady-falcon-66/1ebe3ceb-….jsonl:541` |
| 5= | Partial delivery | 2 | 2 | 0.18% | `task:cas-a487@2026-08-11T13:29:55Z`; `~/.codex/...019fede3….jsonl:624` |
| 5= | Draft/format violation | 1 | 2 | 0.18% | `task:cas-a872@2026-08-07T00:54Z` |
| 7= | Wrong process/environment | 1 | 1 | 0.09% | `~/.codex/...019fe7a0….jsonl:776`; `task:cas-4fa4@2026-08-09T19:02Z` |
| 7= | Scope drift | 1 | 1 | 0.09% | `task:cas-058e@2026-08-08T02:45Z` (originating task `cas-7a21`) |
| 7= | Premature done claim | 1 | 1 | 0.09% | `task:cas-9d92@2026-08-07T21:13:44Z` |

### 4.3 Segment variance

The overall affected-episode rate is 1.85%. Relative to that baseline:

| Harness / model | Sessions | Episodes | Rate | Delta vs 1.85% |
| --- | ---: | ---: | ---: | ---: |
| Claude / Fable 5 | 86 | 6 | 6.98% | +5.13 points |
| Codex / GPT-5.6 Terra | 195 | 9 | 4.62% | +2.77 points |
| Claude / Opus 5 | 275 | 4 | 1.45% | -0.40 points |
| Codex / GPT-5.6 Sol | 460 | 2 | 0.43% | -1.42 points |
| All zero-incident model rows | 119 | 0 | 0.00% | -1.85 points |

This variance is descriptive. Fable’s six episodes are all one guard class over a two-day band; Terra’s nine span four classes and disproportionately include tasks assigned to repair known failures. The table does not establish that either model is intrinsically more failure-prone.

## 5. Proposed guidance edits — not applied

These are proposals for supervisor/operator approval. This task changes no rules or skills.

1. **Add a workspace-path recovery pair to worker guidance.** Six denials affected 6 of 86 Claude Fable 5 sessions (6.98%; 0.53% of all measured sessions). Proposed wording: after a workspace banner, do not retry the same target; route source/build output to the worktree, durable proof to the task artifact root, ephemeral notes to the harness scratchpad, and treat `/dev/null` denial as a guard defect to report rather than inventing another host path. The `/dev/null` and harness-scratchpad rows also justify a runtime-policy follow-up; guidance alone cannot fix false-positive boundaries.

2. **Make the surface checklist demand a pointer, not an assertion.** Four missed-surface episodes affected 4 of 195 GPT-5.6 Terra sessions (2.05%; 0.35% overall). Proposed addition after the existing cas-src checklist: for every applicable row, paste the proving file, command, or test; for every `N/A`, state why. A bare “synced all mirrors” or “migration covered” is not close evidence. This builds on the checklist added by `cas-641f`; it does not rewrite that change during this audit.

3. **Add a crossed-message freshness handshake before corrective work.** Four races affected four Codex sessions (2 Sol, 2 Terra; 0.61% across those 655 sessions). Proposed wording: after a push, `MERGE REQUIRED`, or late amendment, drain the inbox and re-read the task; before producing a corrective commit, test whether the delivered tip is already an ancestor of the target and whether the task is already closed/merged. If yes, re-close or stop instead of editing.

The unscoped-test class produced three raw events in one Claude Opus 5 session. It does not displace the top three because the repository has since added the scoped-test wrapper and the worker guidance already names the passed-count proof. A repeat audit should evaluate the post-guard epoch separately.

## 6. Threats to validity

- **Lower bound, not exhaustive prevalence.** High precision was chosen over recall. Failures described only in euphemistic prose or outside accepted evidence frames are absent.
- **Task mix confounds model comparisons.** Models were deliberately routed by depth/cost, and some sessions were spawned specifically to repair known incidents.
- **Survivorship and reporting bias.** A failure that produced a task note is easier to count than one silently abandoned. Workspace denials are mechanically loud.
- **Mixed episode carriers.** Transcript banners use a session ID; task-note incidents use the worker/task episode associated with the note. Both dedupe repeated prose, but a task handed between workers may undercount unless the note names the originating worker.
- **Historical text inside current notes.** Embedded note timestamps, not `tasks.updated_at`, determine window membership.
- **Model inheritance.** Tool-result rows inherit the session’s last observed model. `<synthetic>` and `unknown` stay explicit rather than being guessed.
- **Append-only source discovery.** A resumed transcript can make an older in-window session visible after the first extraction. The checked-in summary freezes this report's denominator; later live-source reruns should disclose any denominator drift rather than silently rewriting the audit.
- **Grok not measured.** V1 does not support a cross-harness zero claim for Grok.
- **No causal claim.** The variance chart is a routing/observability result, not a benchmark.

What would change the conclusion: adjudicating a second high-recall sample that adds at least seven observed events outside the current top three would overturn the statement that those classes hold the majority. A post-guidance audit with stable routing is required before claiming any edit reduced failures.

## 7. Provenance

- Markdown source: `docs/analysis/2026-08-11-failure-mode-frequency-audit.md`
- HTML review surface: `docs/analysis/2026-08-11-failure-mode-frequency-audit.html`
- Adjudicated manifest: `docs/analysis/2026-08-11-failure-mode-incidents.csv`
- Reproducible candidate miner: `docs/analysis/scripts/mine_failure_modes.py`
- Frozen session summary: `docs/analysis/2026-08-11-failure-mode-session-summary.json`
- Repository commit examined: `bd9c5103` at extraction start
- DB snapshot: `/tmp/cas-935c.sy0M4V/snap.db`; `PRAGMA integrity_check = ok`
- Exact data window: `[2026-07-28T04:00:00Z, 2026-08-11T17:55:35Z)`
- Candidate result: 586 broad lexical candidates → 31 structurally evidence-framed candidates → 21 accepted episodes / 24 raw events
- Session denominator: 437 Claude + 698 Codex = 1,135
- Linked charter: `cas-0cda` (operational intelligence v2); this audit is a one-off, read-only v1 bridge. Recurrence/cron is intentionally left to operator decision.
