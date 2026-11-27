## Core Concepts

### Recorded factory windows

`cas scorecard --since <RFC3339> --until <RFC3339>` (also `cas ledger report`)
reads recorded local facts for `[since, until)`. Both bounds are required and
cannot be combined with `--release`. Explicit windows make no Cloud or GitHub
requests, including when `--offline` is omitted. Use `--json` for automation.

The factory section reports distinct recorded refusal calls per observed merge,
distinct calls by recorded actor (including `unknown`), counts by code,
supervisor wake injection/start median and nearest-rank p90 per
harness, proven agent filings, unattributed filings and unique closed issues in
reconcile receipts updated during the window. Historical refusal facts without
a call id count individually and are identified. Missing actor, harness and wake
start evidence stays visible. Receipt update time is an observation, so the true
issue-close-time count remains unknown. Empty and missing sources are distinct;
zero merges leaves the refusal ratio unknown. These are retained local records,
not an assertion of complete fleet coverage.

Native turn identity survives re-registration. The native scanner persists wake
starts as soon as it sees the authenticated input; it no longer depends on a
diagnostic report to supply that timestamp. New MCP refusal facts include the
authenticated caller, role, harness, tool and action, including uncoded errors.
New issue filings through Cassy retain the caller; historical backfills remain
unattributed.
