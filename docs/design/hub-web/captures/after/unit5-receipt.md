# Unit 5 visual receipt

The pairing captures show the unboxed hero code, ruled detail ledger, status row, and reachable actions. The connection captures show the serif failure verdict, annotated attempt timeline, and Retry/Diagnose actions.

- `unit5-pairing-light-1280.png`, `unit5-pairing-dark-1280.png`: desktop pairing dialog; the `K7MW-4H2Q` code stays on one line.
- `unit5-pairing-light-390.png`, `unit5-pairing-dark-390.png`: phone pairing dialog; the code stays on one line with `white-space: nowrap`.
- `unit5-connection-light-1280.png`, `unit5-connection-dark-1280.png`: desktop failed-connection verdict and evidence timeline.
- `unit5-connection-light-390.png`, `unit5-connection-dark-390.png`: phone failed-connection verdict and evidence timeline without overflow.

Strict visual proof lines:

- PASS `connection-failed-retry`: zero Unit 5 findings in light/dark at 1280/390.
- PASS `pairing-step-1`: zero Unit 5 findings in light/dark at 1280/390.
- PASS `pairing-cleanup`: zero Unit 5 findings in light/dark at 1280/390.
- PASS pairing code metrics: `white-space: nowrap`; height 99px at 1280 and 77.59px at 390 in both schemes.

The full strict matrix also recorded pre-existing findings on `session-canvas` and `attention-12`; those belong to other units and were not allowlisted.
