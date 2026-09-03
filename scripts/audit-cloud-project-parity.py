#!/usr/bin/env python3
"""Per-project local-vs-cloud row parity audit (GH #669 / cas-76a8).

Read-only. For every cloud-linked Cassy project on this host it reports:

  * the canonical id the client resolves (pin -> git remote -> folder name),
  * local row counts per entity type,
  * cloud row counts per entity type for the same project scope,
  * task rows whose persisted ``origin_project`` is a *different spelling* of
    this project, split into the spellings the pre-GH-#669 syntax rule already
    folded and the spellings only the cloud's ``aliases`` record can fold.

The last column is the point of the exercise: those rows are this project's own
history that `cas doctor` and `cas cloud purge-foreign` were counting as another
project's.

Usage:  scripts/audit-cloud-project-parity.py [--json out.json]
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sqlite3
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request

ENTITY_TYPES = [
    "entries",
    "tasks",
    "rules",
    "skills",
    "sessions",
    "verifications",
    "events",
    "prompts",
    "file_changes",
    "commit_links",
    "agents",
    "worktrees",
    "task_dependencies",
]

LOCAL_TABLE_FOR = {
    "entries": "entries",
    "tasks": "tasks",
    "rules": "rules",
    "skills": "skills",
    "events": "events",
    "prompts": "prompts",
    "file_changes": "file_changes",
    "commit_links": "commit_links",
    "task_dependencies": "task_dependencies",
}


def canonicalize(value):
    """Byte-identical twin of cas::cloud::canonical_project_id (GH #669)."""
    if not isinstance(value, str):
        return None
    value = value.strip()
    if not value:
        return None
    value = re.sub(r"^https?://", "", value, flags=re.I)
    value = re.sub(r"^ssh://git@", "", value, flags=re.I)
    value = re.sub(r"^git@([^:]+):", r"\1/", value, flags=re.I)
    value = re.sub(r"/+$", "", value)
    value = re.sub(r"\.git$", "", value, flags=re.I)
    value = re.sub(r"/+$", "", value)
    value = re.sub(r"^/+|/+$", "", value)
    value = value.lower()
    return value or None


def resolve_canonical_id(root: str):
    cfg = os.path.join(root, ".cas", "config.toml")
    if os.path.isfile(cfg):
        with open(cfg, encoding="utf-8", errors="replace") as fh:
            for line in fh:
                m = re.match(r'\s*canonical_id\s*=\s*"([^"]+)"', line)
                if m:
                    return canonicalize(m.group(1)), "config.toml pin"
    try:
        remote = subprocess.run(
            ["git", "-C", root, "remote", "get-url", "origin"],
            capture_output=True, text=True, timeout=15,
        )
        if remote.returncode == 0 and remote.stdout.strip():
            return canonicalize(remote.stdout.strip()), "git remote origin"
    except (OSError, subprocess.SubprocessError):
        pass
    return canonicalize(os.path.basename(os.path.normpath(root))), "folder name"


def config_aliases(root: str):
    cfg = os.path.join(root, ".cas", "config.toml")
    if not os.path.isfile(cfg):
        return []
    with open(cfg, encoding="utf-8", errors="replace") as fh:
        body = fh.read()
    m = re.search(r"^\s*aliases\s*=\s*\[(.*?)\]", body, flags=re.M | re.S)
    if not m:
        return []
    return [canonicalize(v) for v in re.findall(r'"([^"]*)"', m.group(1))]


def local_counts(db_path: str):
    counts, tables = {}, set()
    conn = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True, timeout=10)
    try:
        for (name,) in conn.execute(
            "SELECT name FROM sqlite_master WHERE type='table'"
        ):
            tables.add(name)
        for key, table in LOCAL_TABLE_FOR.items():
            if table in tables:
                counts[key] = conn.execute(f"SELECT COUNT(*) FROM {table}").fetchone()[0]
        origins = []
        if "tasks" in tables:
            cols = {r[1] for r in conn.execute("PRAGMA table_info(tasks)")}
            if "origin_project" in cols:
                origins = [
                    row[0]
                    for row in conn.execute(
                        "SELECT origin_project FROM tasks "
                        "WHERE NULLIF(trim(origin_project), '') IS NOT NULL"
                    )
                ]
        pages = 0
        if "knowledge_pages" in tables:
            pages = conn.execute("SELECT COUNT(*) FROM knowledge_pages").fetchone()[0]
    finally:
        conn.close()
    return counts, origins, pages


def syntax_alias(candidate: str, current: str) -> bool:
    """The pre-GH-#669 rule: a remote-shaped id aliases a bare pin whose value
    is its final path segment."""
    if candidate == current:
        return False
    for a, b in ((candidate, current), (current, candidate)):
        if a.count("/") >= 2 and b.count("/") < 2 and a.rsplit("/", 1)[-1] == b:
            return True
    return False


def cloud_counts(endpoint: str, token: str, project_id: str):
    url = f"{endpoint}/api/sync/pull?project_id={urllib.parse.quote(project_id, safe='')}"
    req = urllib.request.Request(url, headers={"Authorization": f"Bearer {token}"})
    try:
        with urllib.request.urlopen(req, timeout=120) as resp:
            body = json.load(resp)
    except urllib.error.HTTPError as e:
        return {"error": f"HTTP {e.code}"}
    except Exception as e:  # noqa: BLE001 - audit must report, not crash
        return {"error": str(e)}
    return {k: len(v) for k, v in body.items() if isinstance(v, list)}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--json", help="write the full report here")
    args = ap.parse_args()

    with open(os.path.expanduser("~/.cas/cloud.json"), encoding="utf-8") as fh:
        cloud = json.load(fh)
    endpoint, token = cloud["endpoint"].rstrip("/"), cloud["token"]

    listing = subprocess.run(
        ["cas", "known-repos", "list"], capture_output=True, text=True, timeout=60
    ).stdout
    roots = []
    for line in listing.splitlines():
        m = re.match(r"\s*\[ok\s*\]\s+touch_count=\d+\s+(.*)$", line)
        if m and os.path.isfile(os.path.join(m.group(1), ".cas", "cas.db")):
            roots.append(m.group(1))
    roots = sorted(set(roots))

    report = []
    for root in roots:
        canonical, source = resolve_canonical_id(root)
        counts, origins, pages = local_counts(os.path.join(root, ".cas", "cas.db"))
        aliases = config_aliases(root)

        syntax_folded, record_folded, foreign = {}, {}, {}
        for raw in origins:
            c = canonicalize(raw)
            if c is None or c == canonical:
                continue
            if syntax_alias(c, canonical):
                syntax_folded[c] = syntax_folded.get(c, 0) + 1
            elif c in aliases:
                record_folded[c] = record_folded.get(c, 0) + 1
            else:
                foreign[c] = foreign.get(c, 0) + 1

        report.append({
            "root": root,
            "canonical_id": canonical,
            "canonical_id_source": source,
            "registered_aliases": aliases,
            "local": counts,
            "local_knowledge_pages": pages,
            "cloud": cloud_counts(endpoint, token, canonical),
            "task_origin_spellings": {
                "folded_by_syntax_rule": syntax_folded,
                "folded_by_alias_record": record_folded,
                "still_foreign": dict(sorted(
                    foreign.items(), key=lambda kv: -kv[1]
                )[:12]),
                "still_foreign_total": sum(foreign.values()),
                "still_foreign_sources": len(foreign),
            },
        })

    if args.json:
        with open(args.json, "w", encoding="utf-8") as fh:
            json.dump({"projects": report}, fh, indent=2, sort_keys=True)

    for row in report:
        cl = row["cloud"]
        print(f"\n=== {row['canonical_id']}  ({row['root']}, {row['canonical_id_source']})")
        print(f"    registered aliases: {row['registered_aliases'] or '(none)'}")
        if "error" in cl:
            print(f"    cloud: UNAVAILABLE ({cl['error']})")
        for key in ENTITY_TYPES:
            local, remote = row["local"].get(key), cl.get(key)
            if local in (None, 0) and remote in (None, 0):
                continue
            delta = "" if local is None or remote is None else f"  delta={remote - local:+d}"
            print(f"    {key:<20} local={local!s:<8} cloud={remote!s:<8}{delta}")
        o = row["task_origin_spellings"]
        print(f"    task origin spellings: syntax-folded={o['folded_by_syntax_rule']} "
              f"record-folded={o['folded_by_alias_record']} "
              f"foreign={o['still_foreign_total']} rows from {o['still_foreign_sources']} projects")
    return 0


if __name__ == "__main__":
    sys.exit(main())
