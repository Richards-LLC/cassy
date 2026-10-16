#!/usr/bin/env python3
"""Real SQLite pass/breach and receipt-semantics controls for the release row."""
import importlib.util
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('slo_gate', Path(__file__).with_name('check-communication-slos.py'))
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


class CommunicationSlos(unittest.TestCase):
    def test_fixture_passes_all_five(self):
        with gate.fixture() as conn:
            m = gate.measure(conn)
            self.assertEqual([], gate.breaches(m))
            self.assertAlmostEqual(2.0, m['worker_hours'])

    def test_each_breach_fails_the_gate(self):
        for sql, metric in (
            ("UPDATE prompt_queue SET highest_stage='abandoned' WHERE id=1", 'false-abandon'),
            ("UPDATE operator_delivery_outbox SET relay_receipt=NULL,retained_at=NULL", 'operator-relay-receipts'),
            ("UPDATE communication_body_observations SET body_count=2", 'duplicate-body-reads'),
            ("UPDATE agents SET registered_at='2026-01-02T01:30:00Z'", 'director-nudges'),
            ("UPDATE prompt_queue SET transport_delivered_at=NULL WHERE id=1; DELETE FROM prompt_queue_recipient_seen", 'gated-without-outcome'),
        ):
            with self.subTest(metric=metric), gate.fixture() as conn:
                conn.executescript(sql)
                self.assertIn(metric, gate.breaches(gate.measure(conn)))

    def test_gate_process_exits_nonzero_for_each_breach(self):
        cases = (
            ("UPDATE prompt_queue SET highest_stage='abandoned' WHERE id=1;", 'false-abandon'),
            ("UPDATE operator_delivery_outbox SET relay_receipt=NULL,retained_at=NULL;", 'operator-relay-receipts'),
            ("UPDATE communication_body_observations SET body_count=2;", 'duplicate-body-reads'),
            ("UPDATE agents SET registered_at='2026-01-02T01:30:00Z';", 'director-nudges'),
            ("UPDATE prompt_queue SET transport_delivered_at=NULL WHERE id=1; DELETE FROM prompt_queue_recipient_seen;", 'gated-without-outcome'),
        )
        for mutation, metric in cases:
            with self.subTest(metric=metric), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                script = root / 'scripts/check-communication-slos.py'
                script.parent.mkdir(parents=True)
                shutil.copyfile(Path(gate.__file__), script)
                sql_root = root / 'crates/cas-store/src/communication_slo'
                sql_root.mkdir(parents=True)
                for source in gate.SQL_ROOT.glob('*.sql'):
                    shutil.copyfile(source, sql_root / source.name)
                fixture = root / 'scripts/fixtures/communication-slos'
                shutil.copytree(gate.FIXTURE, fixture)
                with (fixture / 'session.sql').open('a') as stream:
                    stream.write(mutation + '\n')
                result = subprocess.run([sys.executable, str(script)], capture_output=True, text=True)
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertIn('BREACH ' + metric, result.stdout)

    def test_transport_is_not_seen_and_ended_unread_is_not_false_abandon(self):
        with gate.fixture() as conn:
            conn.executescript("UPDATE prompt_queue_recipient_seen SET source='transport_delivered'; "
                               "UPDATE prompt_queue SET highest_stage='abandoned' WHERE id=1")
            self.assertEqual(0, gate.measure(conn)['false_abandon'])

    def test_historical_duplicate_coverage_is_unknown(self):
        with gate.fixture() as conn:
            conn.executescript("UPDATE communication_observation_epoch SET started_at='2026-01-03T00:00:00Z'")
            self.assertIn('duplicate-body-reads', gate.breaches(gate.measure(conn)))

    def test_retired_worker_exposure_is_preserved_once(self):
        with gate.fixture() as conn:
            conn.execute("INSERT INTO events(event_type,entity_id,metadata,created_at) VALUES(?,?,?,?)",
                         ('agent_shutdown', 'fixture-worker',
                          '{"retired_agent":{"role":"worker","factory_session":"fixture-session",'
                          '"registered_at":"2026-01-02T00:00:00Z"}}', gate.NOW))
            self.assertAlmostEqual(2.0, gate.measure(conn)['worker_hours'])
            conn.execute('DELETE FROM agents')
            self.assertAlmostEqual(2.0, gate.measure(conn)['worker_hours'])

    def test_session_and_recipient_isolation(self):
        with gate.fixture() as conn:
            conn.execute("INSERT INTO prompt_queue(id,source,target,created_at,factory_session,highest_stage) "
                         "VALUES(99,'director','other','2026-01-02T00:00:00Z','other-session','abandoned')")
            conn.execute("INSERT INTO prompt_queue_recipient_seen VALUES(3,'wrong-recipient',?,'hook_surfaced')", (gate.NOW,))
            self.assertEqual([], gate.breaches(gate.measure(conn)))

    def test_no_events_is_vacuous_and_nudges_without_exposure_breach(self):
        with gate.fixture() as conn:
            conn.execute('DELETE FROM operator_delivery_outbox')
            conn.execute('DELETE FROM agents')
            m = gate.measure(conn)
            self.assertEqual(0, m['operator_events'])
            self.assertEqual(['director-nudges'], gate.breaches(m))


if __name__ == '__main__':
    unittest.main()
