#!/usr/bin/env python3
"""Local stand-in for the Cassy Cloud peer-collaboration API (cas-daa0).

A network-reachable port of cas-cli/tests/distributed_factory_test_cases/
fake_cloud.rs, so the two-machine live check can run before the cloud preview
exists. Same routes and response shapes as the fake, plus what a live run
needs:

- bearer tokens are checked: only the tokens in --tokens-file (one per line)
  are accepted, anything else gets 401;
- leased peer messages return to `queued` when their lease expires;
- every request is appended to --request-log as JSON lines (tokens never
  logged), for the runner's receipts;
- `POST /__standin/faults` (admin token from --admin-token-file) arms
  failures, e.g. {"register_fail": 2} answers the next two registrations
  with 503; `GET /__standin/state` returns the state for receipts.

Usage:
  standin_cloud.py --bind 100.66.32.70 --port 18471 \
      --tokens-file tokens.txt --admin-token-file admin.txt --request-log req.jsonl
"""

import argparse
import hashlib
import json
import sys
import threading
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit


def now_dt():
    return datetime.now(timezone.utc)


def now():
    return now_dt().isoformat()


def normalize_project(project_id):
    """The client's canonical_id comparison, without aliases."""
    return (project_id or "").strip().lower().removesuffix(".git")


class State:
    def __init__(self):
        self.agents = {}
        self.locks = {}
        self.messages = []
        self.faults = {"register_fail": 0}
        self.lock = threading.Lock()

    def snapshot(self):
        return {
            "agents": list(self.agents.values()),
            "locks": {key: lock_json(key, lock) for key, lock in self.locks.items()},
            "messages": self.messages,
            "faults": self.faults,
        }


def lock_json(key, lock):
    return {
        "id": f"lock-{key}",
        "task_id": key,
        "agent_id": lock["agent_id"],
        "status": lock["status"],
        "expires_at": lock["expires_at"],
        "renewed_at": lock["renewed_at"],
        "renewal_count": lock["renewal_count"],
        "epoch": 1,
        "claim_reason": lock["claim_reason"],
    }


def active_locks(state, owner=None):
    return [
        lock_json(key, lock)
        for key, lock in sorted(state.locks.items())
        if lock["status"] == "active" and (owner is None or lock["agent_id"] == owner)
    ]


def expire_leases(state):
    current = now_dt()
    for message in state.messages:
        if message["status"] == "leased" and message.get("lease_until"):
            if datetime.fromisoformat(message["lease_until"]) <= current:
                message["status"] = "queued"
                message["lease_until"] = None


def handle(state, method, path, body):
    segments = [part for part in path.strip("/").split("/")]
    match (method, segments):
        case ("POST", ["api", "agents", "register"]):
            if state.faults.get("register_fail", 0) > 0:
                state.faults["register_fail"] -= 1
                return 503, {"error": "stand-in: injected registration failure"}
            agent_id = body.get("id") or ""
            agent = {
                "id": agent_id,
                "name": body.get("name"),
                "agent_type": body.get("agent_type"),
                "status": "active",
                "pid": body.get("pid"),
                "session_id": None,
                "machine_id": body.get("machine_id"),
                "last_heartbeat": now(),
                "active_tasks": 0,
                "metadata": body.get("metadata"),
            }
            state.agents[agent_id] = agent
            return 200, {"status": "success", "agent": agent}
        case ("GET", ["api", "agents"]):
            return 200, {
                "status": "success",
                "agents": list(state.agents.values()),
                "next_cursor": None,
                "next_cursor_id": None,
            }
        case ("GET", ["api", "agents", "locks"]):
            return 200, {"status": "success", "locks": active_locks(state)}
        case ("GET", ["api", "agents", agent_id, "locks"]):
            return 200, {"status": "success", "locks": active_locks(state, agent_id)}
        case ("POST", ["api", "agents", agent_id, "heartbeat"]):
            agent = state.agents.get(agent_id)
            if agent is None:
                return 404, {"error": "agent not found"}
            agent["last_heartbeat"] = now()
            agent["status"] = "active"
            return 200, agent
        case ("POST", ["api", "agents", agent_id, "shutdown"]):
            if agent_id in state.agents:
                state.agents[agent_id]["status"] = "shutdown"
            released = 0
            for lock in state.locks.values():
                if lock["agent_id"] == agent_id and lock["status"] == "active":
                    lock["status"] = "released"
                    released += 1
            return 200, {"status": "success", "released_locks": released}
        case (_, ["api", "agents", "tasks", key, action]):
            return claim_route(state, method, key, action, body)
        case ("POST", ["api", "peer-messages"]):
            return send(state, body)
        case ("POST", ["api", "peer-messages", "claim"]):
            expire_leases(state)
            recipients = set(body.get("recipient_agent_ids") or [])
            lease = max(1, min(int(body.get("lease_secs") or 120), 3600))
            limit = max(1, min(int(body.get("max") or 50), 500))
            claimed = []
            for message in state.messages:
                if len(claimed) >= limit:
                    break
                if message["recipient_agent_id"] in recipients and message["status"] == "queued":
                    message["status"] = "leased"
                    message["attempts"] += 1
                    message["lease_until"] = (now_dt() + timedelta(seconds=lease)).isoformat()
                    claimed.append(dict(message))
            return 200, {"messages": claimed}
        case ("POST", ["api", "peer-messages", "ack"]):
            for ack in body.get("acks") or []:
                for message in state.messages:
                    if message["id"] == ack.get("id"):
                        message["status"] = ack.get("outcome")
                        message["delivered_at"] = now()
                        message["lease_until"] = None
            return 200, {"status": "success"}
        case ("GET", ["api", "peer-messages", message_id]):
            expire_leases(state)
            for message in state.messages:
                if message["id"] == message_id:
                    return 200, {
                        "id": message["id"],
                        "status": message["status"],
                        "delivered_at": message["delivered_at"],
                        "attempts": message["attempts"],
                    }
            return 404, {"error": "not found"}
    return 404, {"error": "not found"}


def claim_route(state, method, key, action, body):
    agent_id = body.get("agent_id") or ""
    lock = state.locks.get(key)
    active = lock if lock and lock["status"] == "active" else None
    duration = max(1, min(int(body.get("duration_secs") or 600), 3600))
    match (method, action):
        case ("POST", "claim"):
            # As petra-stella-cloud today: an active row is held whatever its
            # expires_at (petra-stella-cloud#150).
            if active and active["agent_id"] != agent_id:
                return 409, {
                    "status": "conflict",
                    "result": None,
                    "lock": None,
                    "error": "already_claimed",
                    "owner_agent_id": active["agent_id"],
                }
            if active:
                return 200, {
                    "status": "success",
                    "result": "claimed",
                    "lock": lock_json(key, active),
                    "error": None,
                    "owner_agent_id": None,
                }
            new_lock = {
                "agent_id": agent_id,
                "status": "active",
                "expires_at": (now_dt() + timedelta(seconds=duration)).isoformat(),
                "renewed_at": now(),
                "renewal_count": 0,
                "claim_reason": body.get("reason"),
            }
            state.locks[key] = new_lock
            return 200, {
                "status": "success",
                "result": "claimed",
                "lock": lock_json(key, new_lock),
                "error": None,
                "owner_agent_id": None,
            }
        case ("POST", "release"):
            if active and active["agent_id"] == agent_id:
                active["status"] = "released"
            return 200, {"status": "success"}
        case ("POST", "renew"):
            if active and active["agent_id"] == agent_id:
                active["expires_at"] = (now_dt() + timedelta(seconds=duration)).isoformat()
                active["renewed_at"] = now()
                active["renewal_count"] += 1
                return 200, {"status": "success", "lock": lock_json(key, active)}
            return 404, {"error": "No active lock found"}
        case ("GET", "lock"):
            return 200, {"lock": lock_json(key, active) if active else None}
    return 404, {"error": "not found"}


def send(state, body):
    sender = state.agents.get(body.get("sender_agent_id") or "")
    recipient = state.agents.get(body.get("recipient_agent_id") or "")
    if sender is None or recipient is None:
        return 404, {"error": "unknown agent"}
    project = body.get("project_id") or ""
    recipient_repo = (recipient.get("metadata") or {}).get("canonical_id") or ""
    if normalize_project(recipient_repo) != normalize_project(project):
        return 403, {"error": "recipient is not in this project"}
    for existing in state.messages:
        if existing["dedupe_key"] == body.get("dedupe_key"):
            return 200, {"id": existing["id"], "status": "duplicate"}
    message_id = f"pm_{len(state.messages) + 1}"
    state.messages.append(
        {
            "id": message_id,
            "project_id": project,
            "recipient_agent_id": body.get("recipient_agent_id"),
            "sender_agent_id": body.get("sender_agent_id"),
            "sender_name": sender.get("name"),
            "sender_machine_id": sender.get("machine_id"),
            "sender_session": (sender.get("metadata") or {}).get("factory_session"),
            "body": body.get("body"),
            "summary": body.get("summary"),
            "in_reply_to": body.get("in_reply_to"),
            "dedupe_key": body.get("dedupe_key"),
            "created_at": now(),
            "attempts": 0,
            "status": "queued",
            "delivered_at": None,
            "lease_until": None,
        }
    )
    return 200, {"id": message_id, "status": "queued"}


def token_fingerprint(token):
    return hashlib.sha256(token.encode()).hexdigest()[:12] if token else None


def make_handler(state, tokens, admin_token, request_log):
    log_lock = threading.Lock()

    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *args):
            pass

        def _reply(self, status, payload):
            data = json.dumps(payload).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def _serve(self, method):
            length = int(self.headers.get("Content-Length") or 0)
            raw = self.rfile.read(length) if length else b""
            try:
                body = json.loads(raw) if raw else {}
            except ValueError:
                body = {}
            path = urlsplit(self.path).path
            auth = self.headers.get("Authorization") or ""
            token = auth.removeprefix("Bearer ").strip() if auth.startswith("Bearer ") else ""
            if path.startswith("/__standin/"):
                if not admin_token or token != admin_token:
                    status, payload = 401, {"error": "admin token required"}
                elif method == "POST" and path == "/__standin/faults":
                    with state.lock:
                        state.faults.update({k: int(v) for k, v in body.items()})
                        status, payload = 200, {"faults": dict(state.faults)}
                elif method == "GET" and path == "/__standin/state":
                    with state.lock:
                        status, payload = 200, state.snapshot()
                else:
                    status, payload = 404, {"error": "not found"}
            elif token not in tokens:
                status, payload = 401, {"error": "invalid or missing token"}
            else:
                with state.lock:
                    status, payload = handle(state, method, path, body)
            if request_log:
                entry = {
                    "at": now(),
                    "method": method,
                    "path": path,
                    "status": status,
                    "token": token_fingerprint(token),
                    "body": body,
                }
                with log_lock, open(request_log, "a") as handle_log:
                    handle_log.write(json.dumps(entry) + "\n")
            self._reply(status, payload)

        def do_GET(self):
            self._serve("GET")

        def do_POST(self):
            self._serve("POST")

    return Handler


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--bind", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=18471)
    parser.add_argument("--tokens-file", required=True)
    parser.add_argument("--admin-token-file", required=True)
    parser.add_argument("--request-log")
    args = parser.parse_args(argv)
    tokens = {line.strip() for line in open(args.tokens_file) if line.strip()}
    admin_token = open(args.admin_token_file).read().strip()
    if not tokens:
        print("no tokens in --tokens-file", file=sys.stderr)
        return 2
    server = ThreadingHTTPServer(
        (args.bind, args.port), make_handler(State(), tokens, admin_token, args.request_log)
    )
    print(f"stand-in cloud listening on http://{args.bind}:{server.server_address[1]}", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
