# Operational intelligence periodic sweep

`docs/analysis/scripts/operational_sweep.py` is the read-only operator command
that joins the M1 evidence namespace, M2 evaluated hybrid retrieval, M3
deployed-binary recurrence verdicts, and the M4 memory contradiction queue.
One invocation emits a Markdown report, structured JSON, stage receipts, and
ready-to-review issue/task drafts beneath its configured artifact root.

The supervisor explicitly chose this Python analysis-lane command over a Rust
`cas sweep` wrapper. A Rust wrapper remains an epic-close follow-up: shipping
one here would either reimplement M1–M4 or make the CAS binary depend on the
repository's `docs/analysis` tree and Python runtime.

## Safety contract

- Every SQLite source is opened with URI `mode=ro` and `PRAGMA query_only=ON`.
- Only `artifact_root` is written. The prior-run watermark is advanced only
  after M2's gate, M3, and M4 all complete successfully.
- M2 failure, unavailable evaluation gate, malformed output, M3 failure, or M4
  failure produces `failed-run.json` and leaves the prior watermark unchanged.
- Only M1 units in `correction_state='current'` are eligible. First observation
  is `MIN(evidence_provenance.timestamp)`, so a pre-fix claim repeated later is
  not relabelled as new. Retention and redaction receipt hashes are carried in
  the M1 watermark card.
- Reports omit source paths and raw stage output. Evidence excerpts are capped
  at 280 characters; subprocess output is byte-counted and hashed.
- The command has no issue/task filing, memory mutation, or automatic model-turn
  route. Every proposal is marked `draft-human-review-required`.

## Run it

Copy `operational-sweep-config.example.json`, replace the absolute paths, and
run:

```bash
python3 docs/analysis/scripts/operational_sweep.py run \
  --config docs/analysis/operational-sweep-config.json
```

Start with `mode: backfill`. The report distinguishes cursor progress from
corpus exhaustion, so a large M1 events backlog cannot masquerade as “no new
evidence.” After every source is drained, switch to `steady-state`.

`operational-sweep.cron.example` is the supervisor-armable hourly path. It is a
template rather than a committed live cron entry: installation changes operator
runtime state and therefore requires the supervisor's explicit action.

## Report contract

Every analytical claim in `report.json` has one or more `evidence_card_ids`.
The Markdown report prints those IDs beside the claim. M3 `recurred` verdicts
and actionable M4 contradiction rows graduate into the same proposal queue as
the two recurring-failure and two instruction-drift probes. A proposal contains
the standard issue body headings, in order: Environment, Repro, Actual,
Expected, Impact, Suggested fix. The paired task spec carries the same evidence
IDs and remains a draft.

Run receipts record stage and total latency, final artifact bytes, model turns,
and M2 retrieval-query count. M2 does not expose provider billing through its
query contract; USD cost is recorded as unknown instead of estimated without
evidence.
