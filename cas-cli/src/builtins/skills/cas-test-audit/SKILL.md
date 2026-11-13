---
name: cas-test-audit
description: Use when auditing a project's tests for flake risk, historical flakes, dead or duplicate coverage, wording/style pins and cost; not for writing one new test.
metadata:
  managed_by: cas
---

# Test value audit

One read-only pass inventories test value, nondeterminism and the project's own failure history. It changes no audited test and never runs the full local suite.

1. **Scope.** Name the project, source revision, runners and task artifact directory. Read its DESIGN.md before producing a human-facing report.
2. **Mine history, then scan once.** Run the shipped portable script (Python standard library; `gh` for history):

   ```bash
   python3 <skill-dir>/scripts/audit.py --root <project> --history --output <task-artifacts>/audit
   ```

   It writes `audit.json` and `REPORT.md`, including file, line, pattern, evidence, confidence and a fix for each candidate. Read [detectors](references/detectors.md) for scope and limits. No project-local copy or extra audit skill.
3. **Complete the bounded history evidence.** Cover about thirty days: CI attempts/reruns, same-SHA or same-tree flips, queue ejections and green requeues, flaky/quarantine issues, skip lists and Cassy notes. The script reads at most five hundred CI runs and one hundred issues per query; report truncation, unavailable sources and the dates actually covered. Read-only fan-out may review separate buckets. Search Cassy for this project's flake/queue incidents and read relevant notes; inspect current runner skip/exclusion configuration. Inspect native receipts for named failures and executed passes on identical source. Feed those observations into the **same invocation** with `--history-evidence <json>`; its shape is `{ "findings": [...], "coverage": {...} }`, with the same finding fields as `audit.json`. Keep original receipts, run/issue URLs and note citations beside the report. Historical findings stay in the report even when source was subsequently fixed/deleted.
4. **Calibrate.** Hand-check up to ten rows per detector, recording held/sample and false positives. Under fifty percent means `noisy, not proposed`. Low-confidence rows remain inventory, never deletion proposals. A gate needs ninety percent precision and explicit operator approval; this audit is never a gate.
5. **Classify and measure.** Review dead/disabled, superseded, duplicate, restated implementation, style/wording pins, slow and flake-prone tests together. Quote both assertion paths for duplicates; cite the retiring commit for superseded tests. Prove dead code with runner inventory/compiler/caller evidence, not test-only references. Follow [ci-timing](references/ci-timing.md) when making a cost claim; otherwise say cost and flake rate are unmeasured. Keep build time separate from test time.
6. **Deliver and stop.** Publish the report, raw JSON and historical evidence as artifacts, with counts per class, precision, unmeasured inputs and decisions from [ledger](references/ledger.md). Link flake-prone findings to the assigned fix-or-delete batch. No cuts during the audit: a separate approved batch names surviving coverage or `none needed: dead/superseded` and measures any promised saving.

A workflow turning green, a green requeue on another SHA or a retry label alone is not proof that a named test flaked. Match native test identity, executed failure and executed pass, plus identical SHA/tree and comparable selection. Preserve first failure evidence; never retry a suite to discover history or turn this audit green.

For timing guesses, fix by waiting on the event, completion or owned resource; use a controlled clock for a timing contract. Repair isolation or delete useless coverage. Style pinning is deleted rather than reworded; preserve the behavioral checks beside it. Exact caller payloads and machine protocol identifiers remain data-integrity checks. Never prescribe retry wrapping, quarantine or a guessed longer sleep.
