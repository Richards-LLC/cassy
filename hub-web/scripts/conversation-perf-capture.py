#!/usr/bin/env python3
"""Read only Commander rows, never pairing/credential tables. Private replay input."""
import argparse
import json
import os
import re
import sqlite3
import time
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("--db", required=True)
parser.add_argument("--session", required=True)
parser.add_argument("--output", required=True)
args = parser.parse_args()
db = sqlite3.connect(Path(args.db).resolve().as_uri() + "?mode=ro", uri=True)
db.row_factory = sqlite3.Row
lane = """((source LIKE 'commander:%' AND operator_verified = 1)
 OR (source = 'terminal' AND target = 'terminal-history' AND processed_at IS NOT NULL)
 OR lower(target) = 'operator')"""
# Explicit columns: no credentials, unrelated queue rows or complete DB snapshot.
columns = "id,source,target,prompt,created_at,factory_session,operator_device_id,operator_label"
raw_count = db.execute("SELECT count(*) FROM prompt_queue WHERE factory_session=?", (args.session,)).fetchone()[0]


def turns(rows):
    messages, replies = [], []
    for row in reversed(rows):
        common = dict(notification_id=row["id"], at=row["created_at"], session=row["factory_session"] or "")
        if row["target"].lower() == "operator":
            try:
                payload = json.loads(row["prompt"])
            except ValueError:
                continue
            if payload.get("schema_version") not in [1, 2]:
                continue
            reply = {**payload, **common}
            reply.pop("schema_version", None)
            # ConversationHistoryReply skips these absent optionals; live
            # OperatorReply deliberately serializes operator_label as null.
            for key in ["operator_label", "reply_to_session"]:
                if reply.get(key) is None:
                    reply.pop(key, None)
            if not reply.get("options"):
                reply.pop("options", None)
            replies.append(reply)
        else:
            match = re.match(r"\[CAS reply: explicitly acknowledges notification_id=(\d+)\]\n", row["prompt"])
            messages.append({**common, "target": row["target"],
                             "text": row["prompt"][match.end():] if match else row["prompt"],
                             "reply_to": int(match[1]) if match else None,
                             "state": "delivered" if row["source"] == "terminal" else "acknowledged",
                             "stamped": row["source"] != "terminal",
                             "device_id": row["operator_device_id"] or "terminal",
                             "operator_label": row["operator_label"] or "Terminal"})
    return dict(messages=messages, replies=replies)


pages, queries, before = [], [], None
while True:
    start = time.perf_counter()
    rows = db.execute(f"SELECT {columns} FROM prompt_queue WHERE factory_session=? AND {lane} "
                      + ("AND id < ? " if before is not None else "") + "ORDER BY id DESC LIMIT 51",
                      (args.session, before) if before is not None else (args.session,)).fetchall()
    queries.append((time.perf_counter() - start) * 1000)
    page = {**turns(rows[:50]), "has_earlier": len(rows) > 50}
    if rows:
        page["next_before"] = rows[:50][-1]["id"]
    if before is None:
        earlier = db.execute(f"SELECT {columns} FROM prompt_queue WHERE (factory_session IS NULL OR factory_session<>?) "
                             f"AND {lane} ORDER BY id DESC LIMIT 30", (args.session,)).fetchall()
        other = turns(earlier)
        page.update(earlier_messages=other["messages"], earlier_replies=other["replies"])
    pages.append(page)
    if not page["has_earlier"]:
        break
    before = page["next_before"]

out = Path(args.output)
out.parent.mkdir(parents=True, exist_ok=True)
capture = dict(session=args.session, raw_queue_rows=raw_count,
               commander_turns=sum(len(p["messages"]) + len(p["replies"]) for p in pages),
               query_ms=queries, pages=pages,
               caveat="Read-only SQL plus protocol replay; Rust serialization, hub auth and real network latency are not measured.")
pending_rows = db.execute(f"SELECT {columns} FROM prompt_queue WHERE factory_session=? "
                          "AND lower(target)='operator' AND processed_at IS NULL "
                          "ORDER BY priority ASC,id ASC", (args.session,)).fetchall()
# Daemon live OperatorReply omits the durable history timestamp/session.
capture["pending_replies"] = []
for row in pending_rows:
    payload = json.loads(row["prompt"])
    if payload.get("schema_version") not in [1, 2]:
        continue
    payload.pop("schema_version", None)
    payload["notification_id"] = row["id"]
    payload.setdefault("operator_label", None)
    if not payload.get("options"):
        payload.pop("options", None)
    capture["pending_replies"].append(payload)
fd = os.open(out, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
with os.fdopen(fd, "w") as stream:
    json.dump(capture, stream)
print(json.dumps({**{k: v for k, v in capture.items() if k not in ["pages", "pending_replies"]},
                  "pending_replies": len(capture["pending_replies"])}))
