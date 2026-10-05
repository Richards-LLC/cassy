#!/usr/bin/env python3
"""Prepare bounded Jev batches and gate their suggestions using frozen evidence."""
import argparse
import copy
from datetime import datetime
import json
import math
from pathlib import Path, PurePosixPath

CONFIDENCE = 0.9
MAX_STATE_BYTES = 48 * 1024
MAX_HUNK_BYTES = 8 * 1024


def encoded(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def date(value):
    try:
        result = datetime.fromisoformat(value.replace("Z", "+00:00"))
        return result if result.tzinfo is not None else None
    except (AttributeError, TypeError, ValueError):
        return None


def source_path(value):
    if not isinstance(value, str) or not value:
        return False
    path = PurePosixPath(value)
    return (not path.is_absolute() and ".." not in path.parts
            and "\\" not in value and not any(part in path.parts for part in
                ("dist", "target", "node_modules", "vendor"))
            and not value.endswith(".min.js"))


def prepare_state(original):
    state = copy.deepcopy(original)
    created = date(state.get("task", {}).get("created_at"))
    candidates = list(state.get("candidate_commits", []))
    if "candidate_commit" in state:
        candidates.append(state["candidate_commit"])
    for candidate in candidates:
        committed = date(candidate.get("committed_at"))
        # Overwrite caller-supplied flags; date ordering is never a model judgment.
        candidate["predates_task"] = (committed < created
                                      if committed and created else None)
        if any(not source_path(path) for path in candidate.get("paths", [])):
            candidate["patch"] = ""
            candidate["excluded"] = True
            state["evidence_complete"] = False
        patch = candidate.get("patch", "")
        if isinstance(patch, str) and len(patch.encode("utf-8")) > MAX_HUNK_BYTES:
            candidate["patch"] = patch.encode("utf-8")[:MAX_HUNK_BYTES].decode(
                "utf-8", errors="ignore")
            candidate["truncated"] = True
            state["evidence_complete"] = False
    for collection in ("current_code", "current_main_snippets"):
        for citation in state.get(collection, []):
            if not isinstance(citation, dict):
                state["evidence_complete"] = False
                continue
            if not source_path(citation.get("path")):
                citation["snippet"] = ""
                citation["excluded"] = True
                state["evidence_complete"] = False
            text = citation.get("snippet", "")
            if isinstance(text, str) and len(text.encode("utf-8")) > MAX_HUNK_BYTES:
                citation["snippet"] = text.encode("utf-8")[:MAX_HUNK_BYTES].decode(
                    "utf-8", errors="ignore")
                citation["truncated"] = True
                state["evidence_complete"] = False
    return state


def signal(value):
    return (isinstance(value, (int, float)) and not isinstance(value, bool)
            and math.isfinite(value) and CONFIDENCE <= value <= 1)


def citation_for(state, verdict):
    target = state.get("target_sha")
    return isinstance(target, str) and bool(target) and any(
        isinstance(cite, dict) and cite.get("supports") == verdict
        and cite.get("sha") == target and source_path(cite.get("path"))
        and isinstance(cite.get("line"), int) and not isinstance(cite["line"], bool)
        and cite["line"] > 0 and isinstance(cite.get("snippet"), str)
        and bool(cite["snippet"].strip())
        and len(cite["snippet"].encode("utf-8")) <= MAX_HUNK_BYTES
        and not cite.get("truncated") and not cite.get("excluded")
        for cite in state.get("current_code", []))


def route(original, evaluation):
    """No lifecycle writes. Evidence annotations still need independent review."""
    state = prepare_state(original)
    verdict = evaluation.get("answers", {}).get("verdict", {})
    proposed = verdict.get("choice", "UNCLEAR")
    result = {"proposed_verdict": proposed, "verdict": "UNCLEAR",
              "reason": "missing, incomplete or low-confidence evidence"}
    if (state.get("evidence_complete") is not True
            or len(encoded(state).encode("utf-8")) > MAX_STATE_BYTES
            or evaluation.get("status") == "unavailable"):
        return result
    # A separate pair judgment can surface a missed umbrella duplicate even
    # when the majority-class classification said VALID.
    duplicate = state.get("duplicate_candidate", {})
    pair_id = duplicate.get("id")
    if (pair_id and pair_id != state.get("task", {}).get("id")
            and signal(duplicate.get("scope_noul"))
            and duplicate.get("task_scope") == state.get("task", {}).get("description")
            and bool(duplicate.get("task_scope"))
            and any(task.get("id") == pair_id
                    and task.get("description") == duplicate.get("other_scope")
                    and bool(task.get("description"))
                    for task in state.get("similar_tasks", []))
            and proposed in ("VALID", "DUPLICATE", "UNCLEAR")):
        result.update(verdict="DUPLICATE", duplicate_of=pair_id,
                      reason="paired scopes meet the review threshold")
        return result
    if not signal(verdict.get("confidence")) or not citation_for(state, proposed):
        return result
    if proposed == "FIXED":
        if not any(candidate.get("integrated_on_target") is True
                   and candidate.get("target_sha") == state.get("target_sha")
                   and bool(candidate.get("sha"))
                   and candidate.get("predates_task") is False
                   and signal(candidate.get("resolution_noul"))
                   and isinstance(candidate.get("patch"), str)
                   and bool(candidate["patch"].strip())
                   and bool(candidate.get("paths"))
                   and all(source_path(path) for path in candidate["paths"])
                   and len(candidate["patch"].encode("utf-8")) <= MAX_HUNK_BYTES
                   and not candidate.get("truncated")
                   and not candidate.get("excluded")
                   for candidate in state.get("candidate_commits", [])):
            return result
    elif proposed == "OBSOLETE":
        if not state.get("retired_premise"):
            return result
    elif proposed != "VALID":
        return result
    result.update(verdict=proposed, reason="current code meets the review threshold")
    return result


def prepare(input_path, directory, tag, mode):
    records = []
    ids = set()
    for line in input_path.read_text().splitlines():
        if not line.strip():
            continue
        row = json.loads(line)
        state = prepare_state(row.get("state", row))
        record_id = row.get("record_id") or state.get("task", {}).get("id")
        if not isinstance(record_id, str) or not record_id or record_id in ids:
            raise ValueError("Every row needs a unique record_id (or task.id)")
        ids.add(record_id)
        records.append({"record_id": record_id, "state": state})
    if not records:
        raise ValueError("Input has no records")
    directory.mkdir(parents=True, exist_ok=False)
    batches = []
    eligible = [i for i, row in enumerate(records)
                if len(encoded(row["state"]).encode("utf-8")) <= MAX_STATE_BYTES
                and row["state"].get("evidence_complete") is True]
    for start in range(0, len(eligible), 50):
        indices = eligible[start:start + 50]
        name = f"batch-{len(batches):04d}"
        (directory / f"{name}.jsonl").write_text("".join(
            encoded({"state": records[i]["state"]}) + "\n" for i in indices))
        batches.append({"name": name, "indices": indices})
    (directory / "manifest.json").write_text(encoded({
        "run_id": tag, "mode": mode, "records": records, "batches": batches}) + "\n")


def stitch(directory):
    manifest = json.loads((directory / "manifest.json").read_text())
    records = manifest["records"]
    evaluations = {}
    for batch in manifest["batches"]:
        responses = json.loads((directory / f"{batch['name']}.answers.json").read_text())
        if not isinstance(responses, list) or len(responses) != len(batch["indices"]):
            raise ValueError("Batch answer count does not match its frozen manifest")
        evaluations.update(zip(batch["indices"], responses))
    results = []
    for index, row in enumerate(records):
        evaluation = evaluations.get(index, {"status": "unavailable",
                                            "reason": "incomplete or over byte cap"})
        result = {"record_id": row["record_id"], "run_id": manifest["run_id"],
                  "request_id": None, "evaluation": evaluation}
        if manifest["mode"] == "classification":
            result.update(route(row["state"], evaluation))
        results.append(result)
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    build = commands.add_parser("prepare")
    build.add_argument("--input", type=Path, required=True)
    build.add_argument("--directory", type=Path, required=True)
    build.add_argument("--tag", required=True)
    build.add_argument("--mode", choices=("classification", "candidate", "duplicate"),
                       default="classification")
    join = commands.add_parser("stitch")
    join.add_argument("--directory", type=Path, required=True)
    args = parser.parse_args()
    if args.command == "prepare":
        prepare(args.input, args.directory, args.tag, args.mode)
    else:
        print(json.dumps(stitch(args.directory), ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
