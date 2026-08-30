# Codemap refresh latency receipt

`scripts/codemap-latency-receipt.sh` is the bounded, no-content-change
rehearsal for `/codemap`. Run it from the disposable worker worktree after the
codemap skill and CI fixes are present:

```bash
scripts/codemap-latency-receipt.sh \
  --artifact /home/pippenz/.cas/artifacts/cas-731e/codemap-latency-receipt.env \
  --github-run-id <completed-actions-run-id> \
  --github-repo Richards-LLC/cassy
```

The script uses the checked-in `CODEMAP.md` as the no-op render candidate. It
scans the current top-level structure, compares the candidate byte-for-byte,
proves freshness, invokes the already-bounded `cas knowledge build
--timeout-secs 90 --max-sources 5`, and checks local commit/push readiness. It
never writes `.claude/CODEMAP.md`; a different render exits non-zero, so a
content-changing codemap commit cannot be created by a no-op rehearsal.

## Budgets and accounting

| Receipt field | Bound | Includes | Excludes |
| --- | ---: | --- | --- |
| `AGENT_CONTROLLED_TOTAL_SECONDS` | `<=300` | structure scan/render, static freshness proof, bounded knowledge build, local commit/push readiness | runner scheduling and GitHub queue time |
| `KNOWLEDGE_BUILD_SECONDS` | `<=90` | one complete knowledge build invocation | all other codemap work |
| `DOCS_ONLY_REQUIRED_COMPUTE_SECONDS` | `<60` | the slower of required `Fast Validation` and `macOS Check` jobs when an Actions run is supplied | GitHub queue, checkout/runner allocation, advisory or heavy jobs |

The local docs-only phase creates a disposable docs-only commit, runs the
shared classifier, and executes `scripts/test-ci-test-tiers.sh`. This is the
deterministic proxy when no Actions run is supplied. Passing
`--github-run-id` replaces that field with the observed maximum duration of
the two required contexts. The script reports the local proxy separately as
`DOCS_ONLY_LOCAL_CONTRACT_SECONDS`.

`GITHUB_QUEUE_SECONDS` is measured from the Actions workflow `createdAt` to the
first non-skipped job `startedAt`. It is external scheduling time, never part
of `AGENT_CONTROLLED_TOTAL_SECONDS` or codemap work. A missing or unavailable
run is reported as `not-requested` or `unavailable`, not as zero.

When `--artifact` is supplied, the command also copies each phase's captured
output to the sibling directory named by `PHASE_LOG_DIR`; this keeps a
nonzero-but-bounded provider result auditable without mixing its diagnostics
into the parseable receipt.

The committed self-test is:

```bash
scripts/test-codemap-latency-receipt.sh
```

It covers identical and changed render candidates, the no-write invariant,
independent knowledge and agent budgets, required-job timing, and separate
GitHub queue timing. `make -C cas-cli test-ci-tiers` runs this self-test with
the other deterministic CI-tier contracts.
