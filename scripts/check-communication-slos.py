#!/usr/bin/env python3
"""Check communication SLOs using production SQL and a checked-in factory session.

Runs in the release gate's existing no-build fast/integration stage. Never opens
an operator's store. The Rust store tests load the same session into real tables.
"""
import argparse
import json
from pathlib import Path
import sqlite3
import time

ROOT = Path(__file__).resolve().parents[1]
SQL_ROOT = ROOT / 'crates/cas-store/src/communication_slo'
FIXTURE = ROOT / 'scripts/fixtures/communication-slos'
NOW = '2026-01-02T02:00:00Z'


def fixture():
    conn = sqlite3.connect(':memory:')
    conn.row_factory = sqlite3.Row
    for path in (FIXTURE / 'tables.sql',
                 *(SQL_ROOT / name for name in ('schema-body.sql', 'schema-epoch.sql', 'schema-summaries.sql', 'schema-prune.sql')),
                 FIXTURE / 'session.sql'):
        conn.executescript(path.read_text())
    return conn


def measure(conn):
    return dict(conn.execute((SQL_ROOT / 'metrics.sql').read_text(),
                             ('fixture-session', NOW)).fetchone())


def breaches(m):
    return [name for name, failed in (
        ('false-abandon', m['false_abandon'] != 0),
        ('operator-relay-receipts', m['operator_receipts'] != m['operator_events']),
        ('duplicate-body-reads', m['duplicate_body_reads'] != 0 or not m['body_coverage_complete']),
        ('director-nudges', m['director_nudges'] > m['worker_hours'] + 1e-8),
        ('gated-without-outcome', m['gated_without_outcome'] != 0),
    ) if failed]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--json', action='store_true')
    args = parser.parse_args()
    start = time.perf_counter()
    with fixture() as conn:
        values = measure(conn)
    failed = breaches(values)
    result = dict(status='BREACH' if failed else 'PASS', stage='fast/integration',
                  elapsed_ms=round((time.perf_counter()-start)*1000, 3),
                  metrics=values, breaches=failed)
    if args.json:
        print(json.dumps(result))
    else:
        print(f"{result['status']} communication-slos: {5-len(failed)}/5 targets; "
              f"stage=fast/integration wall={result['elapsed_ms']:.3f}ms")
        for name in failed:
            print(f'BREACH {name}')
    return bool(failed)


if __name__ == '__main__':
    raise SystemExit(main())
