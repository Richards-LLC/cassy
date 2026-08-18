#!/usr/bin/env python3
"""Build M3's seed-run inputs out of M1 evidence units and M2 candidates.

M3 (``deployed_epoch_verdicts.py``) consumes two JSON artifacts it does not
produce: a reviewable ``--evidence`` corpus and an ``--semantic-evidence`` map
of *evaluated* scores.  M1 (``evidence_units.py``) owns the corpus and M2
(``operational_index.py``) owns the semantic channel, but nothing joined them,
which is why the v2.71 seed run stayed deferred.  This is that join, and
nothing more:

* ``evidence`` — exports M1's typed units (id, first-seen timestamp, source,
  text, structured tags) in M3's exact input shape.  The corpus is deliberately
  NOT filtered by any fix's terms: M3 derives *exposure* from it, so filtering
  would silently shrink the denominator that decides whether a clean-post
  window is observed well enough to call a fix fixed.
* ``candidates`` — for each seed, asks M2 for its semantic candidates and maps
  each one back onto an M1 unit id via ``content_hash`` (the two corpora share
  that key: 7,826 of M1's 32,773 units are also M2 events on this machine).
  The result is a *review* artifact, not a verdict input.
* ``semantic`` — converts human-approved labels over that candidate pool into
  M3's declared-evaluation map, which records *that* a fix was reviewed
  separately from *what* the review found, so "reviewed, nothing matched" is
  expressible instead of collapsing into "nobody looked".

Why the candidate pool is not fed to M3 directly: M2's hybrid scores are
retrieval scores (~0.03 on a real query), not similarities on the 0–1 scale the
seed files' ``semantic_threshold: 0.8`` assumes.  Passing them through would
compare two different scales — every semantic comparison would silently fail
while still flipping M3 out of its "semantic evidence has not been evaluated"
guard, which is the one thing standing between an unobserved fix and a "fixed"
verdict.  Labels are therefore required to be explicit and human-approved, and
the score written into the map is the label's own confidence.

Read-only: every database is opened with ``mode=ro``.  Nothing writes to CAS
memories, knowledge, tasks, or either upstream artifact.  Standard library only.
"""

from __future__ import annotations

import argparse
import json
import sqlite3
import subprocess
import sys
from pathlib import Path
from typing import Any

# M1 marks a unit whose claim a later authority withdrew. Such a unit is not
# usable as standalone evidence (M1's correction-awareness property), so it is
# excluded from the exported corpus and counted in the manifest instead.
CURRENT = "current"


def connect_ro(path: Path) -> sqlite3.Connection:
    """Open ``path`` read-only, and prove it: query_only blocks any write."""
    conn = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    conn.execute("PRAGMA query_only=ON")
    conn.row_factory = sqlite3.Row
    return conn


def load_seeds(path: Path) -> list[dict[str, Any]]:
    seeds = json.loads(path.read_text())
    if not isinstance(seeds, list):
        raise ValueError("seeds JSON must be a list of fixes")
    for seed in seeds:
        for key in ("id", "title", "fix_built_at"):
            if key not in seed:
                raise ValueError(f"seed {seed.get('id', '<no id>')} is missing {key!r}")
    return seeds


def export_evidence(units_db: Path, since: str | None) -> tuple[list[dict[str, Any]], dict[str, Any]]:
    """M1 units → M3's evidence contract, plus a manifest of what was dropped.

    ``timestamp`` is the unit's earliest provenance timestamp: M3 classifies a
    unit into the pre/mixed/post windows by when it was *observed*, and the
    earliest observation is the conservative choice (a unit first seen before a
    fix shipped must not be counted as post-fix evidence).
    """
    conn = connect_ro(units_db)
    rows = conn.execute(
        """
        SELECT u.id AS unit_id,
               u.text AS text,
               u.unit_type AS unit_type,
               COALESCE(u.claim_key, '') AS claim_key,
               u.correction_state AS correction_state,
               u.occurrence_count AS occurrence_count,
               MIN(p.timestamp) AS first_timestamp,
               MIN(p.source_key) AS source_key
          FROM evidence_units u
          JOIN evidence_provenance p ON p.unit_id = u.id
         WHERE p.timestamp <> ''
         GROUP BY u.id
        """
    ).fetchall()
    conn.close()

    evidence: list[dict[str, Any]] = []
    dropped_corrected = 0
    dropped_before_since = 0
    for row in rows:
        if row["correction_state"] != CURRENT:
            dropped_corrected += 1
            continue
        timestamp = row["first_timestamp"]
        if since and timestamp < since:
            dropped_before_since += 1
            continue
        structured = [row["unit_type"].lower()]
        if row["claim_key"]:
            structured.append(row["claim_key"].lower())
        evidence.append(
            {
                "id": str(row["unit_id"]),
                "timestamp": timestamp,
                "source": row["source_key"],
                "text": row["text"],
                "structured": structured,
                "occurrence_count": row["occurrence_count"],
            }
        )

    evidence.sort(key=lambda item: item["timestamp"])
    manifest = {
        "units_db": str(units_db),
        "units_with_timestamps": len(rows),
        "exported": len(evidence),
        "dropped_corrected_or_withdrawn": dropped_corrected,
        "dropped_before_since": dropped_before_since,
        "since": since,
        "first_timestamp": evidence[0]["timestamp"] if evidence else None,
        "last_timestamp": evidence[-1]["timestamp"] if evidence else None,
    }
    return evidence, manifest


def hash_to_unit_id(units_db: Path) -> dict[str, str]:
    conn = connect_ro(units_db)
    mapping = {
        row["content_hash"]: str(row["id"])
        for row in conn.execute("SELECT id, content_hash FROM evidence_units")
    }
    conn.close()
    return mapping


def m2_query(index: Path, script: Path, text: str, top: int, mode: str) -> list[dict[str, Any]]:
    """Ask M2 for candidates through its own CLI.

    Deliberately a subprocess rather than a re-implementation: M2 owns the
    namespace, the gate that authorises a mode, and the provenance contract.
    A non-zero exit (for example an unauthorised mode) is surfaced, never
    swallowed — an unauthorised semantic channel must stop the run, not
    quietly degrade it.
    """
    proc = subprocess.run(
        [
            sys.executable, str(script), "query", text,
            "--index", str(index), "--mode", mode, "--top", str(top),
        ],
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        raise RuntimeError(
            f"M2 query failed (exit {proc.returncode}) for {text!r}: {proc.stderr.strip()[:400]}"
        )
    payload = json.loads(proc.stdout)
    if not payload.get("gate_available", False):
        raise RuntimeError("M2 reports no passing evaluation gate; refusing to build a semantic channel")
    return payload.get("rows", [])


def build_candidates(
    seeds: list[dict[str, Any]], units_db: Path, index: Path, script: Path, top: int, mode: str
) -> dict[str, Any]:
    mapping = hash_to_unit_id(units_db)
    pools: list[dict[str, Any]] = []
    mapped_total = unmapped_total = 0

    for seed in seeds:
        terms = " ".join(seed.get("lexical_terms") or [seed["title"]])
        rows = m2_query(index, script, terms, top, mode)
        candidates = []
        for row in rows:
            unit_id = mapping.get(row.get("content_hash", ""))
            if unit_id:
                mapped_total += 1
            else:
                unmapped_total += 1
            provenance = row.get("provenance") or [{}]
            candidates.append(
                {
                    "event_id": row.get("event_id"),
                    "content_hash": row.get("content_hash"),
                    # None means "M2 knows this event but M1 never made it a
                    # unit" — such a candidate can never become M3 evidence,
                    # and saying so is the point of exporting it.
                    "evidence_id": unit_id,
                    "retrieval_score": row.get("score"),
                    "text": row.get("text", ""),
                    "timestamp": provenance[0].get("timestamp", ""),
                    "task_id": provenance[0].get("task_id", ""),
                    "epoch": provenance[0].get("epoch", ""),
                    "source_path": provenance[0].get("source_path", ""),
                    "label": None,
                    "label_confidence": None,
                    "labelled_by": None,
                    "labelled_at": None,
                }
            )
        pools.append(
            {
                "fix_id": seed["id"],
                "title": seed["title"],
                "query": terms,
                "mode": mode,
                "semantic_threshold": seed.get("semantic_threshold", 0.0),
                "candidates": candidates,
            }
        )

    return {
        "note": (
            "Human review required. `label` must be set to true (symptom recurrence) or false "
            "(not this symptom) with a 0-1 `label_confidence` and a `labelled_by` reviewer before "
            "these become M3 semantic evidence. Labelling every candidate false is a real, "
            "publishable result — it is recorded as an evaluation with zero positives, not as "
            "silence. Retrieval scores are NOT similarities and must not be used as thresholds."
        ),
        "index": str(index),
        "units_db": str(units_db),
        "mapped_candidates": mapped_total,
        "unmapped_candidates": unmapped_total,
        "pools": pools,
    }


def semantic_from_labels(labels: dict[str, Any]) -> tuple[dict[str, dict[str, Any]], dict[str, Any]]:
    """Approved labels → M3's declared-evaluation map.

    Each pool with at least one reviewed candidate emits
    ``{"evaluated": true, "candidates_reviewed": n, "reviewer": ..., "scores": {...}}``.
    Emitting the envelope even when ``scores`` is empty is the point: a review
    that rejected every candidate is a real result, and the bare map shape
    could only report it as silence, which M3 must read as "nobody looked".

    Only ``label is True`` contributes a score, and only when the candidate
    maps to an evidence id. A fix with a positive label M3 cannot attach to an
    evidence unit is withheld entirely rather than published as "evaluated,
    nothing matched" — that would state the opposite of what the reviewer found.
    """
    reviewed: dict[str, dict[str, Any]] = {}
    positives = negatives = unlabelled = unmapped = 0
    withheld: list[str] = []

    for pool in labels.get("pools", []):
        fix_id = str(pool["fix_id"])
        entry = reviewed.setdefault(fix_id, {"scores": {}, "reviewers": set(), "reviewed": 0, "reviewed_at": None})
        for candidate in pool.get("candidates", []):
            label = candidate.get("label")
            if label is None:
                unlabelled += 1
                continue
            reviewer = str(candidate.get("labelled_by") or "").strip()
            if not reviewer:
                raise ValueError(
                    f"{fix_id}: candidate {candidate.get('event_id')} is labelled without `labelled_by`"
                )
            entry["reviewers"].add(reviewer)
            entry["reviewed"] += 1
            entry["reviewed_at"] = max(
                filter(None, [entry["reviewed_at"], str(candidate.get("labelled_at") or "") or None]),
                default=None,
            )
            if not label:
                negatives += 1
                continue
            evidence_id = candidate.get("evidence_id")
            confidence = candidate.get("label_confidence")
            if confidence is None:
                raise ValueError(
                    f"{fix_id}: candidate {candidate.get('event_id')} is labelled true without a confidence"
                )
            if not evidence_id:
                unmapped += 1
                if fix_id not in withheld:
                    withheld.append(fix_id)
                continue
            positives += 1
            entry["scores"][str(evidence_id)] = float(confidence)

    semantic: dict[str, dict[str, Any]] = {}
    for fix_id, entry in reviewed.items():
        if not entry["reviewed"] or fix_id in withheld:
            continue
        semantic[fix_id] = {
            "evaluated": True,
            "candidates_reviewed": entry["reviewed"],
            "reviewer": ", ".join(sorted(entry["reviewers"])),
            "reviewed_at": entry["reviewed_at"],
            "scores": entry["scores"],
        }

    report = {
        "fixes_evaluated": len(semantic),
        "fixes_with_positive_evidence": sum(1 for entry in semantic.values() if entry["scores"]),
        "fixes_evaluated_with_zero_positives": sum(1 for entry in semantic.values() if not entry["scores"]),
        "fixes_withheld_unmappable_positive": withheld,
        "positive_labels": positives,
        "negative_labels": negatives,
        "unlabelled_candidates": unlabelled,
        "positive_but_unmapped": unmapped,
    }
    return semantic, report


def write_json(path: Path, payload: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)

    evidence = sub.add_parser("evidence", help="export M1 units in M3's --evidence shape")
    evidence.add_argument("--units-db", type=Path, required=True)
    evidence.add_argument("--since", help="drop units first observed before this RFC3339 instant")
    evidence.add_argument("--output", type=Path, required=True)
    evidence.add_argument("--output-manifest", type=Path)

    candidates = sub.add_parser("candidates", help="per-seed M2 candidate pool for human labelling")
    candidates.add_argument("--seeds", type=Path, required=True)
    candidates.add_argument("--units-db", type=Path, required=True)
    candidates.add_argument("--index", type=Path, required=True)
    candidates.add_argument(
        "--m2-script",
        type=Path,
        default=Path(__file__).with_name("operational_index.py"),
    )
    candidates.add_argument("--top", type=int, default=20)
    candidates.add_argument("--mode", default="hybrid", help="must be a mode M2's gate authorises")
    candidates.add_argument("--output", type=Path, required=True)

    semantic = sub.add_parser("semantic", help="approved labels → M3's --semantic-evidence map")
    semantic.add_argument("--labels", type=Path, required=True)
    semantic.add_argument("--output", type=Path, required=True)
    semantic.add_argument("--output-report", type=Path)

    args = parser.parse_args(argv)

    if args.command == "evidence":
        units, manifest = export_evidence(args.units_db, args.since)
        write_json(args.output, units)
        if args.output_manifest:
            write_json(args.output_manifest, manifest)
        print(json.dumps(manifest, indent=2, sort_keys=True))
        return 0

    if args.command == "candidates":
        pools = build_candidates(
            load_seeds(args.seeds), args.units_db, args.index, args.m2_script, args.top, args.mode
        )
        write_json(args.output, pools)
        print(
            json.dumps(
                {
                    "pools": len(pools["pools"]),
                    "mapped_candidates": pools["mapped_candidates"],
                    "unmapped_candidates": pools["unmapped_candidates"],
                },
                indent=2,
                sort_keys=True,
            )
        )
        return 0

    semantic_map, report = semantic_from_labels(json.loads(args.labels.read_text()))
    write_json(args.output, semantic_map)
    if args.output_report:
        write_json(args.output_report, report)
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":  # pragma: no cover
    raise SystemExit(main())
