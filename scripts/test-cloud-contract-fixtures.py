#!/usr/bin/env python3
"""Self-test for the recorded cloud contract fixtures (cas-2911).

Validates cas-cli/tests/fixtures/cloud_contract/*.json: the schema, coverage
of every route and error code the 3.51 client depends on, the recorded status
of each documented case, redaction, and the teardown verification. It also
unit-tests the capture script's normaliser offline.

  python3 -I scripts/test-cloud-contract-fixtures.py [--fixtures DIR] [--token-file cloud.json]

`--token-file` additionally scans for the exact token value (read-only).
"""

import argparse
import importlib.util
import json
import re
import sys
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
FIXTURES = REPO / "cas-cli" / "tests" / "fixtures" / "cloud_contract"
CAPTURE = REPO / "scripts" / "cloud-contract-capture.py"

# Every exchange cas-c7c7 needs, per group, with the status production gave.
REQUIRED = {
    "agents": {
        "register_a": 200, "register_b": 200, "register_c_idle": 200,
        "heartbeat_with_metadata": 200, "heartbeat_without_body": 200,
        "list_project_page_1": 200, "list_project_page_2": 200,
        "list_project_page_2_without_sort_cursor": 200, "list_project_page_3": 200,
        "list_project_past_end": 200, "list_project_active": 200,
        "list_project_stale": 200, "list_unknown_project": 200, "list_without_project": 200,
    },
    "claims": {
        "claim": 200, "reclaim_same_agent": 200, "claim_conflict": 409,
        "claim_with_canonical_id": 409, "renew": 200, "get_lock": 200, "list_locks": 200,
        "list_agent_locks": 200, "claim_short": 200, "claim_takeover_after_expiry": 200,
        "renew_after_takeover": 404, "release": 200, "release_again": 200,
        "release_holder": 200, "get_lock_after_release": 200,
    },
    "peer_messages": {
        "send": 200, "send_duplicate": 200, "send_same_key_different_body": 200,
        "send_dedupe_key_conflict": None, "send_sender_not_owned": 403,
        "send_invalid_request_empty_project": 400, "send_unknown_project": 404,
        "send_invalid_project": None, "send_invalid_project_url": None,
        "send_recipient_not_found": 404, "send_invalid_in_reply_to": 400,
        "send_invalid_request_summary": 400, "send_invalid_request_body": 400,
        "send_reply": 200, "claim_invalid_lease": 400, "claim": 200, "claim_reply": 200,
        "status_leased": 200, "ack_invalid_id": 400, "ack": 200, "ack_again": 200,
        "status_delivered": 200, "status_stranger": 404,
    },
    "teardown": {"verification": None},
}
# The error `code` each documented refusal carries.
ERROR_CODES = {
    ("peer_messages", "send_sender_not_owned"): "sender_not_owned",
    ("peer_messages", "send_unknown_project"): "recipient_not_found",
    ("peer_messages", "send_recipient_not_found"): "recipient_not_found",
    ("peer_messages", "send_invalid_in_reply_to"): "invalid_in_reply_to",
    ("peer_messages", "send_invalid_request_summary"): "invalid_request",
    ("peer_messages", "send_invalid_request_body"): "invalid_request",
    ("peer_messages", "send_invalid_request_empty_project"): "invalid_request",
    ("peer_messages", "ack_invalid_id"): "invalid_request",
}
SECRET_LIKE = re.compile(r"[A-Za-z0-9_\-.]{24,}")
EMAIL = re.compile(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}")
TOKEN_FILE = None


def load(group, root=None):
    return json.loads(((root or FIXTURES) / f"{group}.json").read_text())


def strings(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, list):
        for item in value:
            yield from strings(item)
    elif isinstance(value, dict):
        for key, item in value.items():
            yield key
            yield from strings(item)


def by_name(document):
    return {exchange["name"]: exchange for exchange in document["exchanges"]}


class FixtureSchema(unittest.TestCase):
    def test_every_group_has_the_schema(self):
        for group in REQUIRED:
            document = load(group)
            self.assertEqual(document["schema"], 1, group)
            self.assertEqual(document["group"], group)
            for key in ("captured_on", "endpoint", "capture_script", "exchanges"):
                self.assertIn(key, document, f"{group}: {key}")
            self.assertRegex(document["captured_on"], r"^\d{4}-\d{2}-\d{2}$")
            for exchange in document["exchanges"]:
                for key in ("group", "name", "note", "expect", "request", "response"):
                    self.assertIn(key, exchange, f"{group}/{exchange.get('name')}: {key}")
                self.assertEqual(exchange["group"], group)
                response = exchange["response"]
                self.assertIn("status", response)
                self.assertIn("body", response)
                request = exchange["request"]
                if request is not None:
                    self.assertIn(request["method"], ("GET", "POST"))
                    self.assertTrue(request["path"].startswith("/api/"), request["path"])
                    self.assertIsInstance(response["status"], int)

    def test_every_route_and_code_is_covered_with_its_recorded_status(self):
        for group, required in REQUIRED.items():
            exchanges = by_name(load(group))
            for name, status in required.items():
                self.assertIn(name, exchanges, f"{group}/{name} missing")
                if status is not None:
                    self.assertEqual(exchanges[name]["response"]["status"], status, f"{group}/{name}")
                    self.assertEqual(exchanges[name]["expect"], status, f"{group}/{name} expect")

    def test_documented_refusals_carry_their_error_code(self):
        for (group, name), code in ERROR_CODES.items():
            body = by_name(load(group))[name]["response"]["body"]
            self.assertEqual(body.get("code"), code, f"{group}/{name}: {body}")

    def test_contract_relations_hold(self):
        claims = by_name(load("claims"))
        conflict = claims["claim_conflict"]["response"]["body"]
        for key in ("error", "owner_agent_id", "agent_id", "expires_at"):
            self.assertIn(key, conflict)
        # Production's 409 keeps the legacy envelope: status "conflict", result and lock null.
        self.assertEqual(conflict["status"], "conflict")
        self.assertEqual(conflict["error"], "already_claimed")
        self.assertEqual(conflict["owner_agent_id"], "<agent-a>")
        self.assertEqual(conflict["agent_id"], "<agent-a>", "agent_id names the holder, not the caller")
        first = claims["claim"]["response"]["body"]["lock"]
        again = claims["reclaim_same_agent"]["response"]["body"]["lock"]
        self.assertEqual(first["expires_at"], again["expires_at"], "a same-agent re-claim does not extend")
        self.assertEqual(conflict["expires_at"], first["expires_at"])
        self.assertEqual(claims["claim_takeover_after_expiry"]["response"]["body"]["lock"]["agent_id"], "<agent-b>")
        self.assertIsNone(claims["get_lock_after_release"]["response"]["body"]["lock"])
        messages = by_name(load("peer_messages"))
        sent = messages["send"]["response"]["body"]
        self.assertEqual(sent["status"], "queued")
        duplicate = messages["send_duplicate"]["response"]["body"]
        self.assertEqual((duplicate["id"], duplicate["status"]), (sent["id"], "duplicate"))
        claimed = messages["claim"]["response"]["body"]
        self.assertIn("denied", claimed)
        self.assertEqual(claimed["messages"][0]["sender_session"], "<factory-session>")
        results = messages["ack"]["response"]["body"]["results"]
        self.assertEqual([r["status"] for r in results][-1], 404, "an unknown id is a per-item 404")
        again = messages["ack_again"]["response"]["body"]["results"]
        self.assertEqual(again[0]["status"], 409, "a re-ack is a per-item 409")
        agents = by_name(load("agents"))
        stale = agents["list_project_stale"]["response"]["body"]["agents"]
        self.assertIn("<agent-c>", [agent["id"] for agent in stale])
        self.assertTrue(all(agent["status"] == "stale" for agent in stale))
        self.assertEqual(agents["list_unknown_project"]["response"]["body"]["agents"], [])
        # The client's loop echoes only cursor and cursor_id: that page matches the full echo.
        self.assertEqual(agents["list_project_page_2"]["response"]["body"],
                         agents["list_project_page_2_without_sort_cursor"]["response"]["body"])
        self.assertNotIn("sort_cursor", agents["list_project_page_2_without_sort_cursor"]["request"]["query"])
        # A full last page still names a next cursor; the end is an empty page with null cursors.
        self.assertTrue(agents["list_project_page_3"]["response"]["body"]["next_cursor"])
        end = agents["list_project_past_end"]["response"]["body"]
        self.assertEqual(end["agents"], [])
        self.assertIsNone(end["next_cursor"])
        self.assertIsNone(end["next_cursor_id"])

    def test_teardown_was_verified(self):
        checks = by_name(load("teardown"))["verification"]["response"]["body"]
        for key in ("no_scratch_locks", "all_agents_shutdown", "nothing_queued",
                    "messages_terminal", "credential_copy_deleted"):
            self.assertIs(checks.get(key), True, f"teardown check {key}: {checks}")

    def test_nothing_secret_survives(self):
        token = None
        if TOKEN_FILE:
            token = json.loads(Path(TOKEN_FILE).read_text()).get("token")
        for path in sorted(FIXTURES.glob("*.json")):
            text = path.read_text()
            document = json.loads(text)
            public = {document.get("capture_script", ""),
                      re.sub(r"^https?://", "", document.get("endpoint", "")).rstrip("/")}
            if token:
                self.assertNotIn(token, text, f"{path.name}: the exact token survived")
            # Only wire payloads can carry a credential; names and notes are ours.
            payloads = [[e["request"], e["response"]] for e in document["exchanges"]]
            for value in strings(payloads):
                if value.startswith("Bearer "):
                    self.assertEqual(value, "Bearer <redacted-token>", path.name)
                for match in SECRET_LIKE.findall(value):
                    if len(set(match)) >= 10 and not any(match in item for item in public):
                        self.fail(f"{path.name}: secret-like value {match[:6]}… survived")
                self.assertIsNone(EMAIL.search(value), f"{path.name}: an email survived")


class Normaliser(unittest.TestCase):
    def setUp(self):
        spec = importlib.util.spec_from_file_location("capture", CAPTURE)
        self.capture = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.capture)

    def test_equal_values_share_placeholders_and_secrets_go_first(self):
        n = self.capture.Normaliser()
        n.secret("tok_SECRET", "<redacted-token>")
        n.name("cc-abc12345-a", "<agent-a>")
        n.name("cc-abc12345", "cc-<run>")
        doc = {"Authorization": "Bearer tok_SECRET", "id": "cc-abc12345-a", "other": "cc-abc12345-x",
               "t1": "2026-10-13T22:39:53.041Z", "t2": "2026-10-13T22:39:53.041Z",
               "t3": "2026-10-13T22:40:00+00:00",
               "u": "0F1E2D3C-4B5A-6978-8796-A5B4C3D2E1F0", "v": "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0"}
        out = n.apply(doc)
        self.assertEqual(out["Authorization"], "Bearer <redacted-token>")
        self.assertEqual(out["id"], "<agent-a>")
        self.assertEqual(out["other"], "cc-<run>-x")
        self.assertEqual(out["t1"], out["t2"])
        self.assertNotEqual(out["t1"], out["t3"])
        self.assertEqual(out["u"], out["v"])

    def test_claim_key_matches_the_rust_client(self):
        # cas-cli/src/cloud/peer_claims.rs claim_key: `<task>~<8 hex of sha256(project)>`.
        key = self.capture.claim_key("github.com/acme/widget", "cas-1234")
        self.assertRegex(key, r"^cas-1234~[0-9a-f]{8}$")


def main():
    global FIXTURES, TOKEN_FILE
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixtures", type=Path, default=FIXTURES)
    parser.add_argument("--token-file", default=None)
    args, rest = parser.parse_known_args()
    FIXTURES, TOKEN_FILE = args.fixtures, args.token_file
    result = unittest.main(argv=[sys.argv[0], *rest], exit=False, verbosity=2).result
    return 0 if result.wasSuccessful() else 1


if __name__ == "__main__":
    sys.exit(main())
