#!/usr/bin/env python3
"""Render honest deployed-binary symptom-recurrence verdicts.

This is the batch/report companion to ``cas history verdict``.  It deliberately
requires a *deployed* binary epoch before it can describe a fix as fixed: a
landing commit or release tag is only an anchor for finding that epoch, never
evidence that the fix was running.

The inputs are intentionally portable JSON/SQLite artifacts.  This makes a
historical run reviewable and, importantly, lets the v2.71 seed refresh after a
normal daemon restart without modifying the evidence source or filing issues.
"""

from __future__ import annotations

import argparse
import json
import sqlite3
from collections import Counter
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable


DEFAULT_THRESHOLD = 100


def parse_time(value: str | None) -> datetime | None:
    if not value:
        return None
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00")).astimezone(timezone.utc)
    except ValueError:
        return None


def iso(value: datetime | None) -> str | None:
    return value.isoformat().replace("+00:00", "Z") if value else None


@dataclass(frozen=True)
class Epoch:
    started_at: datetime
    ended_at: datetime
    binary_mtime: datetime | None
    version: str | None
    exe_deleted: bool = False

    @property
    def trusted_mtime(self) -> datetime | None:
        return None if self.exe_deleted else self.binary_mtime


@dataclass(frozen=True)
class Boundary:
    fix_started_running: datetime | None
    clean_post_from: datetime | None
    overlapping_nonfixed_epochs: int
    epochs_considered: int
    epochs_without_binary_identity: int

    def classify(self, when: datetime) -> str | None:
        if not self.fix_started_running or not self.clean_post_from:
            return None
        if when < self.fix_started_running:
            return "clean-pre"
        if when < self.clean_post_from:
            return "mixed"
        return "clean-post"


@dataclass(frozen=True)
class Evidence:
    id: str
    timestamp: datetime
    text: str
    source: str
    structured: tuple[str, ...] = ()


def deployed_boundary(epochs: Iterable[Epoch], fix_built_at: datetime) -> Boundary:
    """Match the conservative Rust ``history::epochs::boundary_for`` rule."""
    rows = list(epochs)
    fixed = [row for row in rows if row.trusted_mtime and row.trusted_mtime >= fix_built_at]
    if not fixed:
        return Boundary(None, None, 0, len(rows), sum(row.trusted_mtime is None for row in rows))
    first = min(row.started_at for row in fixed)
    not_fixed = [row for row in rows if not (row.trusted_mtime and row.trusted_mtime >= fix_built_at)]
    overlapping = [row for row in not_fixed if row.ended_at > first]
    clean = max([first, *(row.ended_at for row in overlapping)])
    return Boundary(first, clean, len(overlapping), len(rows), sum(row.trusted_mtime is None for row in rows))


def load_epochs(path: Path) -> list[Epoch]:
    db = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    try:
        rows = db.execute(
            "SELECT started_at, ended_at, binary_mtime, version, exe_deleted "
            "FROM history_epochs WHERE epoch_kind='daemon_start' ORDER BY started_at"
        ).fetchall()
    except sqlite3.Error as exc:
        raise ValueError(f"{path} has no readable history_epochs data; restart an epoch-capable cas serve first") from exc
    finally:
        db.close()
    result = []
    for started, ended, mtime, version, deleted in rows:
        start = parse_time(started)
        if not start:
            continue
        # A start observation is the only honest liveness point when no later
        # heartbeat was recorded; never invent an open-ended daemon lifetime.
        result.append(Epoch(start, parse_time(ended) or start, parse_time(mtime), version, bool(deleted)))
    return result


def load_evidence(path: Path) -> list[Evidence]:
    raw = json.loads(path.read_text())
    if not isinstance(raw, list):
        raise ValueError("evidence JSON must be a list")
    result = []
    for index, item in enumerate(raw):
        stamp = parse_time(str(item.get("timestamp", "")))
        if not stamp:
            continue
        structured = item.get("structured", [])
        result.append(Evidence(
            str(item.get("id", index)), stamp, str(item.get("text", "")),
            str(item.get("source", "unknown")), tuple(str(x).lower() for x in structured),
        ))
    return result


def load_semantic(path: Path | None) -> dict[str, dict[str, float]]:
    if not path:
        return {}
    raw = json.loads(path.read_text())
    if not isinstance(raw, dict):
        raise ValueError("semantic evidence JSON must map each fix id to its evaluated evidence-id → score map")
    return {
        str(fix_id): {str(evidence_id): float(score) for evidence_id, score in scores.items()}
        for fix_id, scores in raw.items() if isinstance(scores, dict)
    }


def render_card(evidence: Evidence, epoch_class: str, channels: list[str], score: float | None) -> dict[str, Any]:
    return {
        "evidence_id": evidence.id,
        "source": evidence.source,
        "timestamp": iso(evidence.timestamp),
        "epoch_class": epoch_class,
        "channels": channels,
        "semantic_score": score,
        "excerpt": evidence.text[:400],
    }


def verdict_for(fix: dict[str, Any], epochs: list[Epoch], evidence: list[Evidence], semantic: dict[str, dict[str, float]]) -> dict[str, Any]:
    built_at = parse_time(str(fix["fix_built_at"]))
    if not built_at:
        raise ValueError(f"{fix['id']}: fix_built_at must be RFC3339")
    boundary = deployed_boundary(epochs, built_at)
    lexical = [term.lower() for term in fix.get("lexical_terms", [])]
    structured = [term.lower() for term in fix.get("structured_terms", [])]
    semantic_threshold = float(fix.get("semantic_threshold", 0.0))
    threshold = int(fix.get("sample_threshold", DEFAULT_THRESHOLD))
    semantic_scores = semantic.get(str(fix["id"]), {})
    cards: list[dict[str, Any]] = []
    exposure = Counter()
    for item in evidence:
        epoch_class = boundary.classify(item.timestamp)
        if not epoch_class:
            continue
        exposure[epoch_class] += 1
        lexical_hit = any(term in item.text.lower() for term in lexical)
        structured_hit = bool(set(structured).intersection(item.structured))
        score = semantic_scores.get(item.id)
        semantic_hit = score is not None and score >= semantic_threshold
        channels = (["structured"] if structured_hit else []) + (["lexical"] if lexical_hit else []) + (["semantic"] if semantic_hit else [])
        if channels:
            cards.append(render_card(item, epoch_class, channels, score))
    post_cards = [card for card in cards if card["epoch_class"] == "clean-post"]
    if not boundary.fix_started_running:
        state, reason = "insufficient-post-fix-data", "no deployed binary epoch contains the fix"
    elif post_cards:
        state, reason = "recurred", "matching symptom evidence occurred in the clean-post window"
    elif not semantic_scores:
        state, reason = "insufficient-post-fix-data", "M2 semantic evidence has not been evaluated for this fix"
    elif exposure["clean-post"] >= threshold:
        state, reason = "fixed", "no symptom match in sufficient clean-post exposure"
    else:
        state, reason = "insufficient-post-fix-data", "clean-post exposure is below the required threshold"
    proposal = None
    if state == "recurred":
        proposal = {
            "kind": "draft-only-recurrence-proposal",
            "title": f"Possible recurrence: {fix['title']}",
            "body": "Human review required. This artifact never files an issue or task.\n\n" +
                    f"Clean-post boundary: {iso(boundary.clean_post_from)}\nEvidence: " +
                    ", ".join(card["evidence_id"] for card in post_cards),
        }
    return {
        "id": fix["id"], "title": fix["title"], "fix_commit": fix.get("fix_commit"),
        "fix_built_at": iso(built_at), "state": state, "reason": reason,
        "epoch_evidence": {
            "fix_started_running": iso(boundary.fix_started_running),
            "clean_post_from": iso(boundary.clean_post_from),
            "overlapping_nonfixed_epochs": boundary.overlapping_nonfixed_epochs,
            "epochs_considered": boundary.epochs_considered,
            "epochs_without_binary_identity": boundary.epochs_without_binary_identity,
        },
        "exposure": dict(clean_pre=exposure["clean-pre"], mixed=exposure["mixed"], clean_post=exposure["clean-post"], threshold=threshold),
        "matching": {"structured_terms": structured, "lexical_terms": lexical, "semantic_scores_supplied": bool(semantic_scores), "semantic_threshold": semantic_threshold},
        "evidence_cards": cards,
        "proposal_draft": proposal,
    }


def markdown(results: list[dict[str, Any]], source: str) -> str:
    lines = ["# Deployed-binary symptom-recurrence report", "",
             f"Input evidence: `{source}`. Generated at {iso(datetime.now(timezone.utc))}.", "",
             "Merge or tag times are anchors only. A verdict uses the first observed daemon epoch carrying the fix and excludes its mixed window.", "",
             "| Fix | Verdict | Clean-post boundary | Exposure |", "| --- | --- | --- | --- |"]
    for item in results:
        epoch = item["epoch_evidence"]
        exposure = item["exposure"]
        lines.append(f"| {item['id']} — {item['title']} | **{item['state']}** | {epoch['clean_post_from'] or 'not observed'} | {exposure['clean_post']}/{exposure['threshold']} clean-post observations |")
    for item in results:
        lines.extend(["", f"## {item['id']} — {item['title']}", "", item["reason"], "",
                      "```json", json.dumps({"epoch_evidence": item["epoch_evidence"], "exposure": item["exposure"], "evidence_cards": item["evidence_cards"], "proposal_draft": item["proposal_draft"]}, indent=2), "```"])
    return "\n".join(lines) + "\n"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--seeds", type=Path, required=True, help="claimed fixes and symptom signatures")
    parser.add_argument("--epochs-db", type=Path, required=True, help="epoch-capable cas.db")
    parser.add_argument("--evidence", type=Path, required=True, help="reviewable timestamped evidence JSON")
    parser.add_argument("--semantic-evidence", type=Path, help="M2 evaluated evidence-id → score JSON")
    parser.add_argument("--output-json", type=Path, required=True)
    parser.add_argument("--output-report", type=Path, required=True)
    args = parser.parse_args()
    seeds = json.loads(args.seeds.read_text())
    if not isinstance(seeds, list):
        raise SystemExit("seed JSON must be a list")
    results = [verdict_for(fix, load_epochs(args.epochs_db), load_evidence(args.evidence), load_semantic(args.semantic_evidence)) for fix in seeds]
    args.output_json.write_text(json.dumps({"verdicts": results}, indent=2) + "\n")
    args.output_report.write_text(markdown(results, str(args.evidence)))


if __name__ == "__main__":
    main()
