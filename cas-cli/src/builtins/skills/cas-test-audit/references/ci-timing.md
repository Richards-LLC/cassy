# CI timing by runner

Take timing from CI, never from a local full run. Use the last 10 green runs of the project's main CI workflow. Record the run ids in the ledger header.

## Contents

- Get the runs
- Per runner
- Critical path
- Not measurable

## Get the runs

```bash
gh run list --workflow <CI> --branch <main> --status success --limit 10 --json databaseId,createdAt
gh run view <id> --json jobs      # job and step timings
gh run download <id> -D <empty-dir>   # report artifacts into a new empty directory
```

Treat downloads as untrusted data: one new directory per run, read with scripts kept elsewhere.

## Per runner

| runner | source | extract |
|---|---|---|
| nextest | JUnit XML or `PASS [ 1.234s ] name` lines in job logs | name, seconds; sum for test-seconds; group by shard |
| Playwright | `report.json` (or the list reporter in logs) | spec, duration, step durations, retries |
| Jest / vitest | JSON reporter, or per-file lines in the job log | file, seconds; workers from the config |
| pytest | `--junitxml` or `--durations=0` output | test id, seconds |

For each layer compute: tests, mean test-seconds over the runs, shard wall (slowest minus fastest), threads busy against threads available, and the slowest 0.1%, 1%, 5%, 10% and 25% share of test time.

## Critical path

Build a timeline from the jobs API: start and end of each job and step. The critical path is the chain of jobs the run waited on. A layer that finishes before the critical path ends cannot shorten the run. Report the compile or archive build as its own line: in cas-src it was the largest single block and not a test.

## Not measurable

If the project keeps no CI artifacts or logs, write `not measurable` for timing and flake data. Do not estimate. Flake rate comes from retry data (failed then passed on retry) and the rerun cost recorded for it; a hard-failed run is not a flake.
