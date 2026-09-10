# Release gate timing receipt

| Field | Contract |
| --- | --- |
| First two lines | Preserve the existing release-gate receipt and version; each executed row keeps its PASS/FAIL verdict ahead of timing evidence. |
| Scannable | Each existing verdict gains a UTC interval and a short line of wall, user CPU and system CPU seconds; a reused row names its source commit and whether the receipt came from the gate or the last green assembly sweep. |
| Readable | Existing failure tails retain the command's cause and status; timing does not replace diagnostic output. |
| Machine output | No new JSON CLI is introduced. `timing.tsv` has stable columns: row, started_utc, ended_utc, wall_s, user_s, system_s, status, source_sha. Executed status is a numeric exit code; a cache hit is REUSED. |
| Omitted | Successful raw command logs and visual-QA captures live in the unique attempt's row directory, with only row verdicts and timing printed in the gate log. Environment values never appear in cache receipts; live precondition rows remain uncached. |

The added lines are ASCII, uncoloured, and fit 80 columns for ordinary timing
values. Existing full command descriptions and absolute checkout paths are
legacy log output, outside this additive change's rendering scope.

## Critique

`terminal-qa: PASS release-gate-timing · 11 runs · 0 fail · 0 warn · 17 allowed`

Receipt: `/home/pippenz/.cas/artifacts/cas-d136/resume/terminal-qa/report.json`.
The five allowlist rules cover unchanged command descriptions, absolute paths,
and the legacy em-dash separator. No new timing line requires an exception.

| Dimension | Score | Evidence |
| --- | ---: | --- |
| Hierarchy | 4 | PASS/FAIL precedes interval and CPU evidence. |
| Fit | 4 | UTC boundaries and explicit seconds match release incident analysis. |
| Craft / distinctiveness | 4 | Three named durations distinguish elapsed time from aggregate CPU. |
| Theme safety | 5 | Added lines are ASCII and contain no colour codes. |
| Machine contract | 5 | Stable TSV carries status and original source SHA separately. |

Scored by bright-raven-48; rechecked by witty-wolf-66 on 2026-09-08 with the same
11-run PASS receipt and unchanged 17 legacy exceptions. All dimensions meet the floor.
