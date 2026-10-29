#!/usr/bin/env python3
"""Read a supervisor-issued exception without changing the failed receipt.

The writer is task action=notes, authenticated against the live agent store.
Match the contract in core/task/fast_rows_exception.rs; legacy skip logs and
caller-supplied approver names are never approval inputs.
"""

import hashlib
import json
from pathlib import Path
import re
import sys


def approved(directory, head):
    if not re.fullmatch(r"[0-9a-f]{40}", head):
        return None
    try:
        failure = (directory / f"{head}.pass").read_bytes()
        lines = failure.decode().splitlines()
        if not lines or not lines[0].startswith(f"fast-rows: FAIL {head} "):
            return None
        if [part for part in lines[0].split() if part.startswith("rows=")] != ["rows=release-notes-draft"]:
            return None
        if any(line.startswith("UNSCOPED ") for line in lines):
            return None
        files = sorted(set(line.removeprefix("SCOPE release-notes-draft ")
                           for line in lines[1:] if line.startswith("SCOPE release-notes-draft ")))
        if not files or any(not re.fullmatch(r"docs/release-notes/[A-Za-z0-9_.-]+-(?:slack|thread)\.md", file)
                            for file in files):
            return None
        digest = hashlib.sha256(failure).hexdigest()
        for path in sorted(directory.glob(f"{head}.*.exception.json")):
            try:
                record = json.loads(path.read_text())
                if not isinstance(record, dict):
                    continue
                if (type(record.get("schema")) is int and record["schema"] == 1 and record.get("head") == head
                        and record.get("check") == "release-notes-draft"
                        and record.get("files") == files
                        and record.get("failure_sha256") == digest
                        and all(isinstance(record.get(key), str) and record[key].strip()
                                for key in ("task_id", "approver", "reason"))):
                    return record
            except (OSError, ValueError, TypeError):
                continue
    except (OSError, ValueError):
        pass
    return None


if __name__ == "__main__":
    record = approved(Path(sys.argv[1]), sys.argv[2])
    if record is None:
        raise SystemExit(1)
    print(f"approved fast-row exception: task={record['task_id']} head={record['head']} "
          f"check={record['check']} files={','.join(record['files'])} "
          f"approver={record['approver']} reason={record['reason']}")
