#!/usr/bin/env python3
"""Capture the production Cassy Cloud contract as recorded fixtures (cas-2911).

Every route the 3.51 client uses (agents, task claims, peer messages) is
exercised once against the real cloud, and each request/response pair is
written to cas-cli/tests/fixtures/cloud_contract/<group>.json. cas-c7c7 makes
the Rust fake clouds match these recordings.

Scope and safety:
- A throwaway project identity, github.com/petrastella-live-check/qa-<run>,
  and scratch agents `cc-<run>-a|b|c`. No real repository or agent is touched.
- The credential is read once, read-only, from --credential (default
  ~/.cas/cloud.json) and copied to a 0600 file under --scratch. Only that copy
  is read afterwards. It is never printed, logged or written to a fixture; the
  copy is deleted at teardown, and a final scan proves no fixture contains it.
- Teardown always runs: every claim is released, every peer message is acked,
  every scratch agent is shut down, and the result is verified and recorded
  in teardown.json.

Normalisation: tokens, the account email and the hostname are redacted, and
ids and timestamps are replaced with stable placeholders (`<agent-a>`,
`<msg-1>`, `<ts-3>`, ...). Equal values map to the same placeholder, so
relations survive (for example, a same-agent re-claim keeps the original
expires_at).

Usage:
  python3 -I scripts/cloud-contract-capture.py            # full capture (~6 min)
  python3 -I scripts/cloud-contract-capture.py --skip-stale --out /tmp/x
Exit status: 0 on a complete capture with verified teardown, 1 if a request
failed unexpectedly or teardown could not be verified.
"""

import argparse
import json
import os
import re
import secrets
import shutil
import socket
import sys
import time
import uuid
import urllib.error
import urllib.parse
import urllib.request
from datetime import datetime, timezone
from hashlib import sha256
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
DEFAULT_OUT = REPO / "cas-cli" / "tests" / "fixtures" / "cloud_contract"
DEFAULT_SCRATCH_ROOT = Path("/mnt/rewind/cas/scratch")
SCHEMA = 1
# A freshly registered agent reads `stale` once its heartbeat is older than
# five minutes; wait a little past that before listing stale agents.
STALE_AFTER_SECS = 310
TIMEOUT_SECS = 20

TS_RE = re.compile(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})")
UUID_RE = re.compile(r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")


def claim_key(project, task_id):
    """The client's claim key: `<task id>~<first 8 hex of sha256(canonical id)>`."""
    return f"{task_id}~{sha256(project.encode()).hexdigest()[:8]}"


class Normaliser:
    """Replace secrets, ids and timestamps with stable placeholders."""

    def __init__(self):
        self.secrets = {}  # exact value -> placeholder, applied first
        self.names = {}  # exact value -> placeholder
        self.counters = {}
        self.by_value = {}

    def secret(self, value, placeholder):
        if value:
            self.secrets[value] = placeholder

    def name(self, value, placeholder):
        if value:
            self.names[value] = placeholder

    def _auto(self, kind, value):
        key = (kind, value)
        if key not in self.by_value:
            self.counters[kind] = self.counters.get(kind, 0) + 1
            self.by_value[key] = f"<{kind}-{self.counters[kind]}>"
        return self.by_value[key]

    def text(self, value):
        for raw, placeholder in self.secrets.items():
            value = value.replace(raw, placeholder)
        # Longest first, so `cc-run-a` is not eaten by `cc-run`.
        for raw in sorted(self.names, key=len, reverse=True):
            value = value.replace(raw, self.names[raw])
        value = TS_RE.sub(lambda m: self._auto("ts", m.group(0)), value)
        value = UUID_RE.sub(lambda m: self._auto("uuid", m.group(0).lower()), value)
        return value

    def apply(self, value):
        if isinstance(value, str):
            return self.text(value)
        if isinstance(value, list):
            return [self.apply(item) for item in value]
        if isinstance(value, dict):
            return {self.text(k): self.apply(v) for k, v in value.items()}
        return value


class Cloud:
    """A minimal HTTP client that records every exchange."""

    def __init__(self, endpoint, token_path):
        self.endpoint = endpoint.rstrip("/")
        self.token_path = token_path
        self.exchanges = []

    def _token(self):
        return self.token_path.read_text().strip()

    def call(self, group, name, method, path, *, query=None, body=None, expect=None,
             note=None, raw_body=None):
        url = self.endpoint + path
        if query:
            url += "?" + urllib.parse.urlencode(query)
        data = None
        headers = {"Authorization": f"Bearer {self._token()}"}
        if raw_body is not None:
            data = raw_body.encode()
            headers["Content-Type"] = "application/json"
        elif body is not None:
            data = json.dumps(body).encode()
            headers["Content-Type"] = "application/json"
        request = urllib.request.Request(url, data=data, method=method, headers=headers)
        try:
            with urllib.request.urlopen(request, timeout=TIMEOUT_SECS) as response:
                status, text = response.status, response.read().decode()
        except urllib.error.HTTPError as error:
            status, text = error.code, error.read().decode()
        try:
            parsed = json.loads(text) if text else None
        except ValueError:
            parsed = {"_non_json": text[:2000]}
        recorded_body = body
        if raw_body is not None:
            recorded_body = {"_oversized_body_bytes": len(raw_body)}
        self.exchanges.append({
            "group": group,
            "name": name,
            "note": note,
            "expect": expect,
            "request": {
                "method": method,
                "path": path,
                "query": query,
                "headers": {k: ("Bearer <redacted-token>" if k == "Authorization" else v)
                            for k, v in headers.items()},
                "body": recorded_body,
            },
            "response": {"status": status, "body": parsed},
        })
        if expect is not None and status != expect:
            print(f"  ! {group}/{name}: expected HTTP {expect}, got {status}", file=sys.stderr)
        else:
            print(f"  {group}/{name}: HTTP {status}")
        return status, parsed


def copy_credential(credential, scratch):
    """Copy the token (and endpoint) into a 0600 scratch file; return both."""
    with open(credential) as handle:
        config = json.load(handle)
    token, endpoint = config.get("token"), config.get("endpoint")
    if not token or not endpoint:
        raise SystemExit(f"{credential}: no token or endpoint")
    scratch.mkdir(parents=True, exist_ok=True, mode=0o700)
    path = scratch / "token"
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as handle:
        handle.write(token)
    return path, endpoint, config.get("email"), token


def register(cloud, agent_id, name, project, session, host, *, label):
    return cloud.call("agents", f"register_{label}", "POST", "/api/agents/register", expect=200, body={
        "id": agent_id,
        "name": name,
        "agent_type": "primary",
        "role": "supervisor",
        "pid": 4242,  # fixed, so fixtures are deterministic
        "parent_id": None,
        "machine_id": host,
        "factory_id": None,
        "clone_path": None,
        "metadata": {
            "role": "supervisor",
            "canonical_id": project,
            "factory_session": session,
            "hostname": host,
        },
    }, note="the 3.51 client's register body (coordinator.rs register_with_focus)")


def capture_agents(cloud, run, project, ids, session, host, registered_at):
    a, b, c = ids
    register(cloud, a, f"cc-{run}-sup-a", project, session, host, label="a")
    register(cloud, b, f"cc-{run}-sup-b", project, session, host, label="b")
    cloud.call("agents", "heartbeat_with_metadata", "POST", f"/api/agents/{a}/heartbeat", expect=200,
               body={"metadata": {"canonical_id": project, "focus": f"cas-{run}e",
                                  "branch": f"factory/cc-{run}", "factory_session": session}},
               note="metadata is merged; canonical_id re-binds, null clears, omitted keeps")
    cloud.call("agents", "heartbeat_without_body", "POST", f"/api/agents/{b}/heartbeat", expect=200,
               note="a bodiless heartbeat still returns 200 and keeps the binding")
    status, page = cloud.call("agents", "list_project_page_1", "GET", "/api/agents",
                              query={"project": project, "limit": 1}, expect=200,
                              note="keyset paging: next_* fields carry the cursor")
    query = {"project": project, "limit": 1}
    for field, param in (("next_cursor", "cursor"), ("next_cursor_id", "cursor_id"),
                         ("next_sort_cursor", "sort_cursor")):
        if isinstance(page, dict) and page.get(field):
            query[param] = page[field]
    cloud.call("agents", "list_project_page_2", "GET", "/api/agents", query=query, expect=200,
               note="second page with the cursor from page 1, project kept")
    cloud.call("agents", "list_project_active", "GET", "/api/agents",
               query={"project": project, "status": "active"}, expect=200)
    cloud.call("agents", "list_unknown_project", "GET", "/api/agents",
               query={"project": f"github.com/petrastella-live-check/none-{run}"}, expect=200,
               note="unknown or unauthorized project: 200 with agents: []")
    cloud.call("agents", "list_without_project", "GET", "/api/agents", query={"limit": 2}, expect=200,
               note="without ?project= only the caller's own agents (truncated to 2 here)")
    return registered_at


def capture_claims(cloud, run, project, a, b):
    t1, t2 = claim_key(project, f"cas-{run}t1"), claim_key(project, f"cas-{run}t2")
    reason = json.dumps({"v": 1, "kind": "task", "name": f"cc-{run}-sup-a", "machine": "<host>",
                         "project": project})
    tasks = f"/api/agents/tasks"
    cloud.call("claims", "claim", "POST", f"{tasks}/{t1}/claim", expect=200,
               body={"agent_id": a, "duration_secs": 600, "reason": reason})
    cloud.call("claims", "reclaim_same_agent", "POST", f"{tasks}/{t1}/claim", expect=200,
               body={"agent_id": a, "duration_secs": 600, "reason": reason},
               note="returns the existing lock and does NOT extend it; use /renew")
    cloud.call("claims", "claim_conflict", "POST", f"{tasks}/{t1}/claim", expect=409,
               body={"agent_id": b, "duration_secs": 600, "reason": None},
               note="409 names owner_agent_id and expires_at; no `status` field")
    cloud.call("claims", "claim_with_canonical_id", "POST", f"{tasks}/{t1}/claim", expect=409,
               body={"agent_id": b, "duration_secs": 600, "canonical_id": project},
               note="a canonical_id in the claim body is ignored")
    cloud.call("claims", "renew", "POST", f"{tasks}/{t1}/renew", expect=200,
               body={"agent_id": a, "duration_secs": 600})
    cloud.call("claims", "get_lock", "GET", f"{tasks}/{t1}/lock", expect=200)
    cloud.call("claims", "list_locks", "GET", "/api/agents/locks", expect=200,
               note="task locks are scoped per user account")
    cloud.call("claims", "list_agent_locks", "GET", f"/api/agents/{a}/locks", expect=200)
    # Takeover: an expired lock is free, and a claim takes it over atomically.
    cloud.call("claims", "claim_short", "POST", f"{tasks}/{t2}/claim", expect=200,
               body={"agent_id": a, "duration_secs": 1, "reason": None})
    time.sleep(2.5)
    cloud.call("claims", "claim_takeover_after_expiry", "POST", f"{tasks}/{t2}/claim", expect=200,
               body={"agent_id": b, "duration_secs": 600, "reason": None},
               note="the expired lock is taken over with a new epoch")
    cloud.call("claims", "renew_after_takeover", "POST", f"{tasks}/{t2}/renew", expect=404,
               body={"agent_id": a, "duration_secs": 600},
               note="404 No active lock found: the original holder lost the claim")
    cloud.call("claims", "release", "POST", f"{tasks}/{t2}/release", expect=200, body={"agent_id": b})
    cloud.call("claims", "release_again", "POST", f"{tasks}/{t2}/release", expect=200,
               body={"agent_id": b}, note="/release is idempotent")
    cloud.call("claims", "release_holder", "POST", f"{tasks}/{t1}/release", expect=200, body={"agent_id": a})
    cloud.call("claims", "get_lock_after_release", "GET", f"{tasks}/{t1}/lock", expect=200,
               note="lock: null once released")
    return [t1, t2]


def capture_peer_messages(cloud, run, project, a, b, consumer):
    pm = "/api/peer-messages"
    send = {"project_id": project, "sender_agent_id": a, "recipient_agent_id": b,
            "body": f"cas-2911 contract capture cc-{run}", "summary": "contract capture",
            "dedupe_key": f"{a}:dedupe-1"}
    _, sent = cloud.call("peer_messages", "send", "POST", pm, expect=200, body=send)
    message_id = sent.get("id") if isinstance(sent, dict) else None
    cloud.call("peer_messages", "send_duplicate", "POST", pm, expect=200, body=send,
               note="status duplicate with the original id")
    cloud.call("peer_messages", "send_same_key_different_body", "POST", pm, expect=200,
               body={**send, "body": "a different body under the same dedupe key"},
               note="same sender and dedupe key: duplicate with the original id, whatever the body")
    cloud.call("peer_messages", "send_dedupe_key_conflict", "POST", pm,
               body={**send, "sender_agent_id": b, "recipient_agent_id": a},
               note="the same dedupe key from another sender of this account. Observed: duplicate with "
                    "the original id. 409 dedupe_key_conflict is documented for a key another account "
                    "used, which one account cannot reach")
    cloud.call("peer_messages", "send_sender_not_owned", "POST", pm, expect=403,
               body={**send, "sender_agent_id": f"cc-{run}-stranger", "dedupe_key": f"{a}:dedupe-2"},
               note="a sender id this account does not own (here: unregistered)")
    cloud.call("peer_messages", "send_invalid_request_empty_project", "POST", pm, expect=400,
               body={**send, "project_id": "", "dedupe_key": f"{a}:dedupe-3"},
               note="an empty project_id is a shape error: invalid_request")
    cloud.call("peer_messages", "send_unknown_project", "POST", pm, expect=404,
               body={**send, "project_id": f"github.com/petrastella-live-check/none-{run}",
                     "dedupe_key": f"{a}:dedupe-8"},
               note="a well-formed project id that resolves to no project of this account reads as "
                    "recipient_not_found (deliberately indistinguishable)")
    cloud.call("peer_messages", "send_invalid_project", "POST", pm,
               body={**send, "project_id": "not a canonical id", "dedupe_key": f"{a}:dedupe-9"},
               note="a malformed project id")
    cloud.call("peer_messages", "send_invalid_project_url", "POST", pm,
               body={**send, "project_id": "https://example.invalid/", "dedupe_key": f"{a}:dedupe-10"},
               note="a URL with no repository path")
    cloud.call("peer_messages", "send_recipient_not_found", "POST", pm, expect=404,
               body={**send, "recipient_agent_id": f"cc-{run}-nobody", "dedupe_key": f"{a}:dedupe-4"},
               note="unknown, wrong project and not addressable are deliberately indistinguishable")
    cloud.call("peer_messages", "send_invalid_in_reply_to", "POST", pm, expect=400,
               body={**send, "in_reply_to": f"cc-{run}-not-a-message", "dedupe_key": f"{a}:dedupe-5"})
    cloud.call("peer_messages", "send_invalid_request_summary", "POST", pm, expect=400,
               body={**send, "summary": "s" * 501, "dedupe_key": f"{a}:dedupe-6"},
               note="summary > 500 chars")
    oversized = json.dumps({**send, "body": "b" * 64001, "dedupe_key": f"{a}:dedupe-7"})
    cloud.call("peer_messages", "send_invalid_request_body", "POST", pm, expect=400,
               raw_body=oversized, note="body > 64000 chars (request body elided in the fixture)")
    reply = {**send, "sender_agent_id": b, "recipient_agent_id": a, "in_reply_to": message_id,
             "body": "reply", "summary": "reply", "dedupe_key": f"{b}:dedupe-1"}
    _, replied = cloud.call("peer_messages", "send_reply", "POST", pm, expect=200, body=reply)
    reply_id = replied.get("id") if isinstance(replied, dict) else None
    cloud.call("peer_messages", "claim_invalid_lease", "POST", f"{pm}/claim", expect=400,
               body={"project_ids": [project], "recipient_agent_ids": [b], "consumer_id": consumer,
                     "max": 50, "lease_secs": 5}, note="lease_secs must be 30-600")
    _, claimed = cloud.call("peer_messages", "claim", "POST", f"{pm}/claim", expect=200,
                            body={"project_ids": [project], "recipient_agent_ids": [b, f"cc-{run}-foreign"],
                                  "consumer_id": consumer, "max": 50, "lease_secs": 120},
                            note="messages plus denied; recipient ids the caller does not own are ignored")
    claimed_ids = [m.get("id") for m in (claimed or {}).get("messages", []) if isinstance(m, dict)]
    _, claimed_a = cloud.call("peer_messages", "claim_reply", "POST", f"{pm}/claim", expect=200,
                              body={"project_ids": [project], "recipient_agent_ids": [a],
                                    "consumer_id": consumer, "max": 50, "lease_secs": 120})
    claimed_ids += [m.get("id") for m in (claimed_a or {}).get("messages", []) if isinstance(m, dict)]
    cloud.call("peer_messages", "status_leased", "GET", f"{pm}/{message_id}", expect=200)
    cloud.call("peer_messages", "ack_invalid_id", "POST", f"{pm}/ack", expect=400,
               body={"consumer_id": consumer,
                     "acks": [{"id": f"cc-{run}-not-a-uuid", "outcome": "delivered"}]},
               note="a non-UUID id rejects the whole request with invalid_request")
    acks = [{"id": mid, "outcome": "delivered"} for mid in claimed_ids if mid]
    acks.append({"id": str(uuid.uuid4()), "outcome": "delivered"})
    cloud.call("peer_messages", "ack", "POST", f"{pm}/ack", expect=200,
               body={"consumer_id": consumer, "acks": acks},
               note="HTTP 200 with per-item results in input order; an unknown id is a per-item 404")
    cloud.call("peer_messages", "ack_again", "POST", f"{pm}/ack", expect=200,
               body={"consumer_id": consumer, "acks": [{"id": message_id, "outcome": "delivered"}]},
               note="per-item 409: not leased to this consumer (already acked)")
    cloud.call("peer_messages", "status_delivered", "GET", f"{pm}/{message_id}", expect=200)
    cloud.call("peer_messages", "status_stranger", "GET", f"{pm}/cc-{run}-not-a-message", expect=404,
               note="404 for anyone but the sender or recipient owner; with one account, an unknown id")
    return [mid for mid in (message_id, reply_id) if mid], claimed_ids


def drain(cloud, project, recipients, consumer, claimed_ids, group="teardown"):
    """Ack every message claimed so far, then claim and reject anything still
    queued for `recipients`. Returns the ids acked here."""
    acked = []
    if claimed_ids:
        cloud.call(group, "ack_claimed", "POST", "/api/peer-messages/ack",
                   body={"consumer_id": consumer,
                         "acks": [{"id": mid, "outcome": "delivered"} for mid in claimed_ids]})
        acked += claimed_ids
    _, left = cloud.call(group, "claim_leftovers", "POST", "/api/peer-messages/claim",
                         body={"project_ids": [project], "recipient_agent_ids": recipients,
                               "consumer_id": consumer, "max": 50, "lease_secs": 30})
    leftovers = [m.get("id") for m in (left or {}).get("messages", []) if isinstance(m, dict)]
    if leftovers:
        cloud.call(group, "ack_leftovers", "POST", "/api/peer-messages/ack",
                   body={"consumer_id": consumer,
                         "acks": [{"id": mid, "outcome": "rejected"} for mid in leftovers]})
        acked += leftovers
    return acked


def teardown(cloud, project, ids, keys, consumer, message_ids, claimed_ids, token_path, scratch):
    """Release, ack, shut down and verify. Returns (verified, checks)."""
    checks = {}
    for key in keys:
        for agent in ids[:2]:
            cloud.call("teardown", f"release_{key[-12:]}_{agent[-1]}", "POST",
                       f"/api/agents/tasks/{key}/release", body={"agent_id": agent})
    # Every claimed message is acked; anything still queued is claimed and rejected.
    drain(cloud, project, ids[:2], consumer, claimed_ids)
    for agent in ids:
        cloud.call("teardown", f"shutdown_{agent[-1]}", "POST", f"/api/agents/{agent}/shutdown")
    _, locks = cloud.call("teardown", "verify_locks", "GET", "/api/agents/locks")
    ours = [l for l in (locks or {}).get("locks", []) if isinstance(l, dict) and l.get("task_id") in keys]
    checks["no_scratch_locks"] = not ours
    _, agents = cloud.call("teardown", "verify_agents", "GET", "/api/agents", query={"project": project})
    rows = [g for g in (agents or {}).get("agents", []) if isinstance(g, dict) and g.get("id") in ids]
    checks["all_agents_shutdown"] = bool(rows) and all(g.get("status") == "shutdown" for g in rows)
    _, again = cloud.call("teardown", "verify_nothing_queued", "POST", "/api/peer-messages/claim",
                          body={"project_ids": [project], "recipient_agent_ids": ids[:2],
                                "consumer_id": consumer, "max": 50, "lease_secs": 30})
    checks["nothing_queued"] = not (again or {}).get("messages")
    statuses = []
    for mid in message_ids:
        _, receipt = cloud.call("teardown", f"verify_message_{len(statuses) + 1}", "GET",
                                f"/api/peer-messages/{mid}")
        statuses.append((receipt or {}).get("status"))
    checks["messages_terminal"] = all(s in ("delivered", "rejected", "expired") for s in statuses)
    token_path.unlink(missing_ok=True)
    shutil.rmtree(scratch, ignore_errors=True)
    checks["credential_copy_deleted"] = not token_path.exists() and not scratch.exists()
    return all(checks.values()), checks


def write_fixtures(out, normaliser, cloud, meta, token, extra_secrets):
    out.mkdir(parents=True, exist_ok=True)
    groups = {}
    for exchange in cloud.exchanges:
        groups.setdefault(exchange["group"], []).append(exchange)
    written = []
    for group, exchanges in groups.items():
        document = normaliser.apply({"schema": SCHEMA, "group": group, **meta, "exchanges": exchanges})
        text = json.dumps(document, indent=2, sort_keys=False) + "\n"
        for value in [token, *extra_secrets]:
            if value and value in text:
                raise SystemExit(f"refusing to write {group}.json: an unredacted secret survived")
        path = out / f"{group}.json"
        path.write_text(text)
        written.append(path)
    return written


def cleanup_run(args):
    """Drain, release and shut down what run `args.cleanup_run` left behind."""
    run = args.cleanup_run
    if not re.fullmatch(r"[0-9a-f]{8}", run):
        raise SystemExit("--cleanup-run takes the 8-hex run id")
    project = f"github.com/petrastella-live-check/qa-{run}"
    scratch = args.scratch or DEFAULT_SCRATCH_ROOT / f"cloud-contract-cleanup-{run}"
    token_path, endpoint, _, token = copy_credential(args.credential, scratch)
    del token
    cloud = Cloud(endpoint, token_path)
    ids = [f"cc-{run}-a", f"cc-{run}-b", f"cc-{run}-c"]
    keys = [claim_key(project, f"cas-{run}t1"), claim_key(project, f"cas-{run}t2")]
    try:
        verified, checks = teardown(cloud, project, ids, keys, f"cc-{run}-consumer", [], [],
                                    token_path, scratch)
    finally:
        token_path.unlink(missing_ok=True)
        shutil.rmtree(scratch, ignore_errors=True)
    print(f"cleanup of run {run}: {'verified' if verified else 'NOT verified'} {checks}")
    return 0 if verified else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--credential", type=Path, default=Path.home() / ".cas" / "cloud.json",
                        help="cloud.json holding endpoint and token (read once, read-only)")
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT)
    parser.add_argument("--scratch", type=Path, default=None,
                        help="scratch directory for the 0600 token copy (deleted at teardown)")
    parser.add_argument("--skip-stale", action="store_true",
                        help="skip the 5-minute wait that captures a live `stale` status")
    parser.add_argument("--cleanup-run", metavar="RUN",
                        help="tear down a previous run by id: drain its messages, release its "
                             "claims and shut its agents down; writes no fixtures")
    args = parser.parse_args()
    if args.cleanup_run:
        return cleanup_run(args)

    run = secrets.token_hex(4)
    project = f"github.com/petrastella-live-check/qa-{run}"
    scratch = args.scratch or DEFAULT_SCRATCH_ROOT / f"cloud-contract-{run}"
    token_path, endpoint, email, token = copy_credential(args.credential, scratch)
    host = socket.gethostname()
    ids = [f"cc-{run}-a", f"cc-{run}-b", f"cc-{run}-c"]
    session = f"cc-{run}-session"
    consumer = f"cc-{run}-consumer"
    cloud = Cloud(endpoint, token_path)

    normaliser = Normaliser()
    normaliser.secret(token, "<redacted-token>")
    normaliser.secret(email, "<redacted-email>")
    for value, placeholder in ((ids[0], "<agent-a>"), (ids[1], "<agent-b>"), (ids[2], "<agent-c>"),
                               (session, "<factory-session>"), (consumer, "<consumer>"),
                               (project, "<project>"), (host, "<host>"),
                               # Run-scoped names keep their shape; a bare run id is never
                               # replaced, so unrelated hex (uuids, digests) stays intact.
                               (f"cc-{run}", "cc-<run>"), (f"cas-{run}", "cas-<run>"),
                               (f"none-{run}", "none-<run>")):
        normaliser.name(value, placeholder)
    for key, placeholder in ((claim_key(project, f"cas-{run}t1"), "<task-key-1>"),
                             (claim_key(project, f"cas-{run}t2"), "<task-key-2>")):
        normaliser.name(key, placeholder)

    print(f"capture run {run} against {endpoint} (project {project})")
    keys, message_ids, claimed_ids, failure = [], [], [], None
    try:
        # Agent c only registers: listed again after five minutes, it reads stale.
        register(cloud, ids[2], f"cc-{run}-idle", project, session, host, label="c_idle")
        registered_at = time.monotonic()
        capture_agents(cloud, run, project, ids, session, host, registered_at)
        keys = capture_claims(cloud, run, project, ids[0], ids[1])
        message_ids, claimed_ids = capture_peer_messages(cloud, run, project, ids[0], ids[1], consumer)
        if not args.skip_stale:
            wait = STALE_AFTER_SECS - (time.monotonic() - registered_at)
            if wait > 0:
                print(f"  waiting {int(wait)}s for agent c's heartbeat to go stale")
                time.sleep(wait)
            cloud.call("agents", "list_project_stale", "GET", "/api/agents",
                       query={"project": project, "status": "stale"}, expect=200,
                       note="status is computed live: a heartbeat older than five minutes reads stale")
    except Exception as error:  # noqa: BLE001 - teardown must still run
        failure = f"{type(error).__name__}: {error}"
        print(f"capture failed: {failure}", file=sys.stderr)
    finally:
        verified, checks = teardown(cloud, project, ids, keys or [claim_key(project, f"cas-{run}t1"),
                                    claim_key(project, f"cas-{run}t2")], consumer, message_ids,
                                    claimed_ids, token_path, scratch)
    cloud.exchanges.append({
        "group": "teardown", "name": "verification", "note": "computed by the capture script",
        "expect": None, "request": None, "response": {"status": None, "body": checks},
    })
    meta = {
        "captured_on": datetime.now(timezone.utc).strftime("%Y-%m-%d"),
        "endpoint": endpoint,
        "capture_script": "scripts/cloud-contract-capture.py",
    }
    written = write_fixtures(args.out, normaliser, cloud, meta, token, [email, host])
    del token
    unexpected = [f"{e['group']}/{e['name']}" for e in cloud.exchanges
                  if e.get("expect") is not None and e["response"]["status"] != e["expect"]]
    print(f"wrote {', '.join(p.name for p in written)}")
    print(f"teardown: {'verified' if verified else 'NOT verified'} {checks}")
    if unexpected:
        print(f"status differed from the expected contract: {', '.join(unexpected)}", file=sys.stderr)
    return 0 if verified and failure is None else 1


if __name__ == "__main__":
    sys.exit(main())
