#!/usr/bin/env python3
"""Run the read-only operational-intelligence sweep (M1 -> M2 -> M3 -> M4).

The command reads the evidence, operational-index, epoch, and memory stores in
read-only mode.  Its only durable writes are beneath ``artifact_root``: one
run directory plus an artifact-local watermark after a completely successful
run.  Findings are proposals for a human reviewer; this module has no issue,
task, memory, or model-turn execution path.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import shlex
import sqlite3
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any, Sequence


DEFAULT_PROBES = (
    {
        "id": "silent-test-suite",
        "family": "recurring_failure",
        "title": "Test command reports success while the intended suite did not run",
        "query": "test suite silently skipped zero tests ran command reported success",
        "expected": "A verification command must prove that the intended test target executed, or fail loudly.",
    },
    {
        "id": "release-claim-drift",
        "family": "recurring_failure",
        "title": "Release claim differs from behavior in the deployed binary",
        "query": "release claimed fixed shipped behavior still old stale running binary",
        "expected": "Release claims must be checked against behavior observed from the deployed binary.",
    },
    {
        "id": "repeated-policy-correction",
        "family": "instruction_drift",
        "title": "Standing instruction repeatedly requires correction",
        "query": "worker repeatedly failed standing rule corrected more than once instruction drift",
        "expected": "One unambiguous standing instruction should produce the same compliant behavior across harnesses.",
    },
    {
        "id": "opposite-policy-interpretation",
        "family": "instruction_drift",
        "title": "Instruction wording permits the opposite interpretation",
        "query": "ambiguous instruction model did opposite operator intended policy conflict",
        "expected": "Policy wording should exclude the unsafe or opposite interpretation.",
    },
)

ISSUE_HEADINGS = ("Environment", "Repro", "Actual", "Expected", "Impact", "Suggested fix")
DB_CURSOR_SPECS = {
    "task": ("tasks", "rowid"),
    "lifecycle_event": ("events", "id"),
    "prompt_queue": ("prompt_queue", "id"),
    "supervisor_queue": ("supervisor_queue", "id"),
}


class SweepError(RuntimeError):
    """A stage could not make an evidence-backed statement."""


def utc_now() -> datetime:
    return datetime.now(timezone.utc)


def iso(value: datetime) -> str:
    return value.isoformat().replace("+00:00", "Z")


def parse_time(value: str | None) -> datetime | None:
    if not value:
        return None
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00")).astimezone(timezone.utc)
    except ValueError:
        return None


def compact(text: str, limit: int = 280) -> str:
    value = " ".join(str(text).split())
    return value if len(value) <= limit else value[: limit - 1] + "…"


def connect_ro(path: Path) -> sqlite3.Connection:
    connection = sqlite3.connect(f"file:{path.resolve()}?mode=ro", uri=True)
    connection.execute("PRAGMA query_only=ON")
    connection.row_factory = sqlite3.Row
    return connection


def write_json(path: Path, payload: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
    temporary.replace(path)


def sha256_text(value: str) -> str:
    return hashlib.sha256(value.encode()).hexdigest()


def size_of(paths: Sequence[Path]) -> int:
    return sum(path.stat().st_size for path in paths if path.exists() and path.is_file())


def table_exists(db: sqlite3.Connection, name: str) -> bool:
    return db.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?", (name,)).fetchone() is not None


def latest_receipts(db: sqlite3.Connection) -> dict[str, list[dict[str, Any]]]:
    result: dict[str, list[dict[str, Any]]] = {"redaction": [], "retention": []}
    if table_exists(db, "redaction_receipts"):
        columns = {row[1] for row in db.execute("PRAGMA table_info(redaction_receipts)")}
        selected = [name for name in ("run_at", "source_key", "secrets", "emails", "receipt_hash") if name in columns]
        if selected:
            rows = db.execute(f"SELECT {','.join(selected)} FROM redaction_receipts ORDER BY rowid DESC LIMIT 20")
            result["redaction"] = [dict(row) for row in rows]
    if table_exists(db, "retention_receipts"):
        columns = {row[1] for row in db.execute("PRAGMA table_info(retention_receipts)")}
        selected = [name for name in ("run_at", "privacy_scope", "cutoff", "provenance_deleted", "units_deleted", "receipt_hash") if name in columns]
        if selected:
            rows = db.execute(f"SELECT {','.join(selected)} FROM retention_receipts ORDER BY rowid DESC LIMIT 20")
            result["retention"] = [dict(row) for row in rows]
    return result


def evidence_rows(units_db: Path, since: str | None, limit: int) -> tuple[list[dict[str, Any]], dict[str, Any]]:
    """Return current, redacted units first observed after ``since``.

    The timestamp is MIN(provenance.timestamp), never a later repeat.  This is
    the same conservative first-observation rule M3 relies on.
    """
    db = connect_ro(units_db)
    try:
        query = """
            SELECT u.id,u.content_hash,u.unit_type,u.text,u.occurrence_count,
                   u.redaction_secrets,u.redaction_emails,u.first_seen_at,
                   COALESCE(MIN(NULLIF(p.timestamp,'')),u.first_seen_at) AS observed_at
            FROM evidence_units u
            LEFT JOIN evidence_provenance p ON p.unit_id=u.id
            WHERE u.correction_state='current'
            GROUP BY u.id
        """
        rows = list(db.execute(query))
        cutoff = parse_time(since)
        selected = []
        for row in rows:
            observed = parse_time(row["observed_at"])
            if cutoff and (not observed or observed <= cutoff):
                continue
            provenance = [
                {
                    "source_key": item["source_key"],
                    "timestamp": item["timestamp"],
                    "task_id": item["task_id"],
                }
                for item in db.execute(
                    "SELECT source_key,timestamp,task_id FROM evidence_provenance "
                    "WHERE unit_id=? ORDER BY timestamp LIMIT 3",
                    (row["id"],),
                )
            ]
            selected.append(
                {
                    "card_id": f"eu:{row['id']}",
                    "content_hash": row["content_hash"],
                    "kind": "evidence_unit",
                    "unit_type": row["unit_type"],
                    "timestamp": row["observed_at"],
                    "occurrence_count": row["occurrence_count"],
                    "excerpt": compact(row["text"]),
                    "redaction": {"secrets": row["redaction_secrets"], "emails": row["redaction_emails"]},
                    "provenance": provenance,
                }
            )
        selected.sort(key=lambda item: item["timestamp"] or "", reverse=True)
        maximum = db.execute(
            "SELECT MAX(COALESCE(NULLIF(p.timestamp,''),u.first_seen_at)) "
            "FROM evidence_units u LEFT JOIN evidence_provenance p ON p.unit_id=u.id "
            "WHERE u.correction_state='current'"
        ).fetchone()[0]
        metadata = {
            "new_units_total": len(selected),
            "returned": min(len(selected), limit),
            "max_current_timestamp": maximum,
            "receipts_honored": latest_receipts(db),
        }
        return selected[:limit], metadata
    finally:
        db.close()


def evidence_by_hash(units_db: Path, hashes: Sequence[str]) -> dict[str, dict[str, Any]]:
    if not hashes:
        return {}
    db = connect_ro(units_db)
    try:
        result = {}
        for content_hash in sorted(set(hashes)):
            row = db.execute(
                "SELECT id,content_hash,unit_type,text,occurrence_count,redaction_secrets,redaction_emails,first_seen_at "
                "FROM evidence_units WHERE content_hash=? AND correction_state='current'",
                (content_hash,),
            ).fetchone()
            if not row:
                continue
            first = db.execute(
                "SELECT source_key,timestamp,task_id FROM evidence_provenance WHERE unit_id=? ORDER BY timestamp LIMIT 1",
                (row["id"],),
            ).fetchone()
            result[content_hash] = {
                "card_id": f"eu:{row['id']}",
                "content_hash": content_hash,
                "kind": "evidence_unit",
                "unit_type": row["unit_type"],
                "timestamp": first["timestamp"] if first else row["first_seen_at"],
                "occurrence_count": row["occurrence_count"],
                "excerpt": compact(row["text"]),
                "redaction": {"secrets": row["redaction_secrets"], "emails": row["redaction_emails"]},
                "provenance": [dict(first)] if first else [],
            }
        return result
    finally:
        db.close()


def backlog_status(units_db: Path) -> dict[str, Any]:
    """Describe watermark progress separately from corpus exhaustion."""
    db = connect_ro(units_db)
    try:
        if not table_exists(db, "ingest_watermarks"):
            return {"status": "unknown", "reason": "ingest_watermarks table absent", "sources": []}
        marks = list(db.execute(
            "SELECT source_key,source_path,cursor_kind,byte_offset,row_cursor,size_bytes,updated_at "
            "FROM ingest_watermarks ORDER BY source_key"
        ))
    finally:
        db.close()
    sources = []
    unknown = 0
    remaining_total = 0
    for mark in marks:
        remaining: int | None = None
        try:
            source = Path(mark["source_path"])
            if mark["cursor_kind"] == "byte-offset":
                remaining = max(0, source.stat().st_size - int(mark["byte_offset"]))
            elif mark["source_key"].startswith("db:"):
                unit_type = mark["source_key"].split(":", 1)[1]
                table, cursor = DB_CURSOR_SPECS[unit_type]
                source_db = connect_ro(source)
                try:
                    maximum = source_db.execute(f"SELECT COALESCE(MAX({cursor}),0) FROM {table}").fetchone()[0]
                finally:
                    source_db.close()
                remaining = max(0, int(maximum) - int(mark["row_cursor"]))
        except (OSError, KeyError, sqlite3.Error, ValueError):
            unknown += 1
        if remaining is not None:
            remaining_total += remaining
        sources.append({
            "source_key": mark["source_key"],
            "cursor_kind": mark["cursor_kind"],
            "remaining": remaining,
            "updated_at": mark["updated_at"],
        })
    status = "drained" if not unknown and remaining_total == 0 else "backlog"
    if unknown and remaining_total == 0:
        status = "unknown"
    return {"status": status, "remaining_total": remaining_total, "unknown_sources": unknown, "sources": sources}


@dataclass
class StageRunner:
    receipts: list[dict[str, Any]]

    def run(self, name: str, command: Sequence[str], outputs: Sequence[Path] = ()) -> dict[str, Any]:
        started = time.monotonic()
        process = subprocess.run(
            list(command), capture_output=True, text=True, check=False,
            env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"},
        )
        elapsed = round((time.monotonic() - started) * 1000, 1)
        receipt = {
            "stage": name,
            "latency_ms": elapsed,
            "exit_code": process.returncode,
            "stdout_bytes": len(process.stdout.encode()),
            "stdout_sha256": sha256_text(process.stdout),
            "stderr_bytes": len(process.stderr.encode()),
            "output_bytes": size_of(outputs),
        }
        self.receipts.append(receipt)
        if process.returncode:
            raise SweepError(f"{name} failed with exit {process.returncode}: {compact(process.stderr, 500)}")
        try:
            return json.loads(process.stdout) if process.stdout.strip() else {}
        except json.JSONDecodeError as exc:
            raise SweepError(f"{name} returned non-JSON output (sha256 {receipt['stdout_sha256']})") from exc


def m2_findings(
    runner: StageRunner, python: str, script: Path, index: Path, units_db: Path,
    probes: Sequence[dict[str, str]], top: int,
) -> tuple[list[dict[str, Any]], dict[str, dict[str, Any]]]:
    payloads = []
    hashes = []
    for probe in probes:
        payload = runner.run(
            f"m2-query:{probe['id']}",
            [python, str(script), "query", probe["query"], "--index", str(index), "--mode", "hybrid", "--top", str(top)],
        )
        if payload.get("gate_available") is not True:
            raise SweepError(f"m2-query:{probe['id']} refused: evaluation gate is unavailable")
        rows = payload.get("rows")
        if not isinstance(rows, list):
            raise SweepError(f"m2-query:{probe['id']} returned no reviewable rows list")
        hashes.extend(str(row.get("content_hash", "")) for row in rows)
        payloads.append((probe, payload))
    mapped = evidence_by_hash(units_db, hashes)
    cards: dict[str, dict[str, Any]] = {}
    findings = []
    for probe, payload in payloads:
        card_ids = []
        candidates = []
        for row in payload["rows"]:
            content_hash = str(row.get("content_hash", ""))
            card = mapped.get(content_hash)
            if not card:
                card = {
                    "card_id": f"m2:{row.get('event_id')}",
                    "kind": "operational_index_candidate",
                    "timestamp": (row.get("provenance") or [{}])[0].get("timestamp"),
                    "occurrence_count": int(row.get("duplicate_count", 1)),
                    "excerpt": compact(row.get("text", "")),
                    "provenance": [
                        {key: item.get(key) for key in ("timestamp", "task_id", "worker")}
                        for item in (row.get("provenance") or [])[:3]
                    ],
                }
            cards[card["card_id"]] = card
            card_ids.append(card["card_id"])
            candidates.append({
                "evidence_card_id": card["card_id"],
                "retrieval_score": row.get("score"),
                "occurrence_count": card.get("occurrence_count", 1),
            })
        findings.append({
            "id": probe["id"],
            "section": probe["family"],
            "title": probe["title"],
            "statement": (
                f"Hybrid retrieval surfaced {len(card_ids)} candidate(s) for “{probe['title']}”; "
                "these are review candidates, not an automated defect verdict."
            ),
            "expected": probe["expected"],
            "evidence_card_ids": card_ids,
            "candidates": candidates,
            "query_latency_ms": payload.get("latency_ms"),
        })
    return findings, cards


def issue_body(finding: dict[str, Any], card: dict[str, Any], invocation: str) -> str:
    values = {
        "Environment": f"Generated by operational_sweep.py on {platform.platform()}; Python {platform.python_version()}.",
        "Repro": f"```bash\n{invocation}\n```",
        "Actual": f"> {card.get('excerpt') or finding['statement']}\n\nEvidence card: `{card['card_id']}`.",
        "Expected": finding["expected"],
        "Impact": "Human triage required. Recurrence and severity have not been inferred automatically.",
        "Suggested fix": "Unknown — inspect the evidence card and linked provenance before tasking.",
    }
    return "\n\n".join(f"## {heading}\n\n{values[heading]}" for heading in ISSUE_HEADINGS) + "\n"


def proposals(findings: Sequence[dict[str, Any]], cards: dict[str, dict[str, Any]], invocation: str) -> list[dict[str, Any]]:
    drafts = []
    for finding in findings:
        if not finding["evidence_card_ids"]:
            continue
        card = cards[finding["evidence_card_ids"][0]]
        drafts.append({
            "proposal_id": f"proposal:{finding['id']}",
            "status": "draft-human-review-required",
            "never_auto_file": True,
            "title": finding["title"],
            "evidence_card_ids": finding["evidence_card_ids"],
            "issue_body": issue_body(finding, card, invocation),
            "task_spec": {
                "title": finding["title"],
                "description": finding["statement"],
                "acceptance_criteria": finding["expected"],
                "evidence_card_ids": finding["evidence_card_ids"],
                "status": "draft-human-review-required",
            },
        })
    return drafts


def add_m3_m4_claims(
    verdicts: dict[str, Any], contradictions: dict[str, Any], cards: dict[str, dict[str, Any]], claims: list[dict[str, Any]],
) -> None:
    for verdict in verdicts.get("verdicts", []):
        evidence_ids = []
        for item in verdict.get("evidence_cards", []):
            card_id = f"m3:{verdict['id']}:{item.get('evidence_id')}"
            cards[card_id] = {"card_id": card_id, "kind": "recurrence_evidence", **item}
            evidence_ids.append(card_id)
        epoch_id = f"epoch:{verdict['id']}"
        cards[epoch_id] = {
            "card_id": epoch_id,
            "kind": "deployed_epoch_boundary",
            "epoch_evidence": verdict.get("epoch_evidence"),
            "exposure": verdict.get("exposure"),
        }
        claims.append({
            "section": "recurrence_verdicts",
            "statement": f"{verdict['id']} is {verdict['state']}: {verdict['reason']}",
            "evidence_card_ids": [epoch_id, *evidence_ids],
        })
    for index, item in enumerate(contradictions.get("items", [])):
        memory = item.get("memory", {})
        verdict = item.get("verdict", {})
        card_id = f"m4:{memory.get('id', index)}:{verdict.get('id', index)}"
        cards[card_id] = {
            "card_id": card_id,
            "kind": "memory_contradiction_review",
            "memory_id": memory.get("id"),
            "claim": memory.get("claim"),
            "link": item.get("link"),
            "verdict": verdict,
            "suggested_action": item.get("suggested_action"),
        }
        claims.append({
            "section": "contradiction_queue",
            "statement": (
                f"Memory {memory.get('id', 'unknown')} is linked to {verdict.get('id', 'an observed fix')}; "
                f"suggested action: {item.get('suggested_action') or 'hold for review'}."
            ),
            "evidence_card_ids": [card_id],
        })


def render_report(payload: dict[str, Any]) -> str:
    lines = [
        "# Operational intelligence periodic sweep", "",
        "> Proposals only. No issue, task, memory operation, or model turn was executed.", "",
        f"Run `{payload['run']['id']}` completed in **{payload['run']['mode']}** mode at {payload['run']['completed_at']}.", "",
        "## Corpus and watermark status", "",
        f"M1 source status: **{payload['backlog']['status']}**; estimated remaining cursor units/bytes: "
        f"{payload['backlog'].get('remaining_total', 'unknown')}. This is reported separately from new evidence since the prior sweep.", "",
        f"New current evidence units since `{payload['run']['since'] or 'first run'}`: "
        f"{payload['new_evidence']['metadata']['new_units_total']} (showing {payload['new_evidence']['metadata']['returned']}).", "",
    ]
    sections = (
        ("recurring_failure", "Top recurring failure narratives"),
        ("instruction_drift", "Instruction-drift clusters"),
        ("recurrence_verdicts", "Deployed-binary recurrence verdicts"),
        ("contradiction_queue", "Memory contradiction queue"),
    )
    for key, title in sections:
        lines.extend([f"## {title}", ""])
        section_claims = [claim for claim in payload["claims"] if claim["section"] == key]
        if not section_claims:
            lines.extend(["No evidence-backed claim was produced for this section.", ""])
        for claim in section_claims:
            cards = ", ".join(f"`{item}`" for item in claim["evidence_card_ids"])
            lines.extend([f"- {claim['statement']} Evidence: {cards}.", ""])
    lines.extend(["## Proposal review queue", ""])
    for proposal in payload["proposals"]:
        lines.extend([
            f"### {proposal['title']}", "",
            "Status: **draft — explicit human review required; never auto-filed**.", "",
            proposal["issue_body"],
        ])
    if not payload["proposals"]:
        lines.extend(["No draft met the minimum evidence-card requirement.", ""])
    cost = payload["run"]["cost"]
    lines.extend([
        "## Run receipt", "",
        f"- Latency: {payload['run']['latency_ms']} ms.",
        f"- Storage: {payload['run']['storage_bytes']} bytes beneath the artifact run directory.",
        f"- Cost: {cost['model_turns']} model turns; {cost['m2_queries']} M2 retrieval queries. "
        "The M2 contract does not expose provider billing, so USD cost is recorded as unknown rather than invented.",
        "- Stage stdout is hashed and byte-counted in `report.json`; it is not copied into this report.", "",
        "## Evidence cards", "",
        "See `report.json` for the structured, redacted evidence-card map. Excerpts are capped at 280 characters; source paths and raw prompt/log payloads are omitted.", "",
    ])
    return "\n".join(lines)


def load_config(path: Path) -> dict[str, Any]:
    payload = json.loads(path.read_text())
    if not isinstance(payload, dict):
        raise SweepError("config must be a JSON object")
    base = path.resolve().parent
    path_keys = {
        "artifact_root", "units_db", "index", "epochs_db", "memories_db", "seeds",
        "semantic_evidence", "m2_script", "m3_script", "m4_script", "join_script",
    }
    for key in path_keys.intersection(payload):
        if payload[key] is None:
            continue
        candidate = Path(payload[key]).expanduser()
        payload[key] = str(candidate if candidate.is_absolute() else base / candidate)
    return payload


def validate_paths(config: dict[str, Any]) -> None:
    required = ("artifact_root", "units_db", "index", "epochs_db", "memories_db", "seeds")
    missing = [key for key in required if not config.get(key)]
    if missing:
        raise SweepError(f"config missing required key(s): {', '.join(missing)}")
    artifact_root = Path(config["artifact_root"]).resolve()
    for key in required[1:]:
        source = Path(config[key]).resolve()
        if not source.is_file():
            raise SweepError(f"{key} is not a readable file: {source}")
        if source == artifact_root or artifact_root in source.parents:
            raise SweepError(f"input {key} must not live beneath artifact_root")


def run(config: dict[str, Any], argv: Sequence[str]) -> Path:
    validate_paths(config)
    started_at = utc_now()
    started = time.monotonic()
    artifact_root = Path(config["artifact_root"]).resolve()
    artifact_root.mkdir(parents=True, exist_ok=True)
    state_path = artifact_root / "state.json"
    prior_state = json.loads(state_path.read_text()) if state_path.exists() else {}
    since = config.get("since") or prior_state.get("last_evidence_timestamp")
    mode = config.get("mode", "steady-state")
    if mode not in {"backfill", "steady-state"}:
        raise SweepError("mode must be backfill or steady-state")
    run_id = started_at.strftime("%Y%m%dT%H%M%SZ")
    run_dir = artifact_root / "runs" / run_id
    if run_dir.exists():
        run_id += f"-{os.getpid()}"
        run_dir = artifact_root / "runs" / run_id
    run_dir.mkdir(parents=True)
    runner = StageRunner([])
    python = str(config.get("python", sys.executable))
    script_dir = Path(__file__).resolve().parent
    units_db = Path(config["units_db"])
    index = Path(config["index"])
    seeds = Path(config["seeds"])
    m2_script = Path(config.get("m2_script", script_dir / "operational_index.py"))
    m3_script = Path(config.get("m3_script", script_dir / "deployed_epoch_verdicts.py"))
    m4_script = Path(config.get("m4_script", script_dir / "memory_contradictions.py"))
    join_script = Path(config.get("join_script", script_dir / "seed_evidence_inputs.py"))
    try:
        new_cards, new_metadata = evidence_rows(units_db, since, int(config.get("new_evidence_limit", 100)))
        backlog = backlog_status(units_db)
        probes = config.get("probes", DEFAULT_PROBES)
        findings, cards = m2_findings(
            runner, python, m2_script, index, units_db, probes, int(config.get("top", 5))
        )
        cards.update({card["card_id"]: card for card in new_cards})
        claims = [
            {
                "section": finding["section"],
                "statement": finding["statement"],
                "evidence_card_ids": finding["evidence_card_ids"],
            }
            for finding in findings if finding["evidence_card_ids"]
        ]
        verdict_path = run_dir / "m3-verdicts.json"
        contradiction_path = run_dir / "m4-contradictions.json"
        with tempfile.TemporaryDirectory(dir=run_dir, prefix="private-stage-") as temporary:
            evidence_path = Path(temporary) / "evidence.json"
            window_start = config.get("window_start") or iso(started_at - timedelta(days=int(config.get("window_days", 7))))
            runner.run(
                "m1-export",
                [python, str(join_script), "evidence", "--units-db", str(units_db), "--since", window_start,
                 "--output", str(evidence_path)],
                [evidence_path],
            )
            m3_command = [
                python, str(m3_script), "--seeds", str(seeds), "--epochs-db", str(config["epochs_db"]),
                "--evidence", str(evidence_path), "--output-json", str(verdict_path),
                "--output-report", str(Path(temporary) / "m3-report.md"),
            ]
            if config.get("semantic_evidence"):
                m3_command.extend(["--semantic-evidence", str(config["semantic_evidence"])])
            runner.run("m3-verdicts", m3_command, [verdict_path])
        verdicts = json.loads(verdict_path.read_text())
        runner.run(
            "m4-contradictions",
            [python, str(m4_script), "queue", "--memories-db", str(config["memories_db"]),
             "--seeds", str(seeds), "--verdicts", str(verdict_path), "--output", str(contradiction_path)],
            [contradiction_path],
        )
        contradictions = json.loads(contradiction_path.read_text())
        add_m3_m4_claims(verdicts, contradictions, cards, claims)
        invocation = " ".join(shlex.quote(part) for part in argv)
        drafts = proposals(findings, cards, invocation)
        report_json = run_dir / "report.json"
        report_md = run_dir / "report.md"
        payload = {
            "run": {
                "id": run_id,
                "mode": mode,
                "since": since,
                "started_at": iso(started_at),
                "completed_at": iso(utc_now()),
                "latency_ms": round((time.monotonic() - started) * 1000, 1),
                "storage_bytes": 0,
                "cost": {"model_turns": 0, "m2_queries": len(probes), "reported_usd": None},
                "stage_receipts": runner.receipts,
            },
            "backlog": backlog,
            "new_evidence": {"cards": [card["card_id"] for card in new_cards], "metadata": new_metadata},
            "findings": findings,
            "claims": claims,
            "proposals": drafts,
            "evidence_cards": cards,
            "contradiction_counts": contradictions.get("counts", {}),
        }
        write_json(report_json, payload)
        report_md.write_text(render_report(payload))
        payload["run"]["storage_bytes"] = size_of(path for path in run_dir.rglob("*") if path.is_file())
        write_json(report_json, payload)
        report_md.write_text(render_report(payload))
        state = {
            "last_completed_run": run_id,
            "last_completed_at": payload["run"]["completed_at"],
            "last_evidence_timestamp": new_metadata.get("max_current_timestamp") or since,
            "mode": mode,
            "backlog_status": backlog["status"],
        }
        write_json(state_path, state)
        return report_md
    except Exception as exc:
        write_json(run_dir / "failed-run.json", {
            "run_id": run_id,
            "failed_at": iso(utc_now()),
            "error": compact(str(exc), 800),
            "stage_receipts": runner.receipts,
            "state_advanced": False,
        })
        raise


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    run_parser = sub.add_parser("run", help="produce one artifact-only sweep report")
    run_parser.add_argument("--config", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        config = load_config(args.config)
        invocation = [sys.executable, str(Path(__file__).resolve()), *(argv or sys.argv[1:])]
        report = run(config, invocation)
    except (OSError, ValueError, SweepError, sqlite3.Error, json.JSONDecodeError) as exc:
        print(f"operational sweep failed: {exc}", file=sys.stderr)
        return 1
    print(json.dumps({"status": "complete", "report": str(report)}, sort_keys=True))
    return 0


if __name__ == "__main__":  # pragma: no cover
    raise SystemExit(main())
