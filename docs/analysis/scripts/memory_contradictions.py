#!/usr/bin/env python3
"""Surface memories that observed behaviour contradicts or confirms (M4, cas-2332).

A memory is a point-in-time claim. The store already has the levers to age one
out — ``valid_until``, importance/stability, ``opinion_reinforce`` /
``opinion_weaken`` / ``opinion_contradict`` — but nothing connected those levers
to what was later observed, so a memory saying "X is broken" keeps being
retrieved with full confidence years after X was fixed. (On this machine, 3 of
1,383 live memories carry a ``valid_until`` at all.)

This builds a *review queue*: (memory, claim, evidence, suggested opinion-op)
triples with full provenance, from M3's deployed-binary verdicts and M1's
evidence units. Three rules keep it honest:

* **No automatic mutation, ever.** ``queue`` opens every store read-only.
  ``apply`` is dry-run by default and refuses to execute anything that is not
  explicitly approved by a named approver. The mutation itself goes through the
  existing ``cas memory`` opinion ops, so the memory system's own audit trail
  records the outcome rather than this script writing behind its back.
* **Silence on unobserved data.** A verdict of ``insufficient-post-fix-data``
  proposes nothing. "We did not observe a recurrence" and "we observed no
  recurrence over adequate exposure" are different statements, and only the
  second one licenses ageing a memory out.
* **Every item is auditable.** Each row carries the link channel that connected
  memory to fix (task id, commit, or matched phrase), the verdict's epoch
  boundary and exposure, and the evidence card ids — so a reviewer can disagree
  with the machine from the same facts it used.

Standard library only.
"""

from __future__ import annotations

import argparse
import json
import re
import sqlite3
import subprocess
import sys
from pathlib import Path
from typing import Any, Callable

# A memory that asserts a defect exists is the class M4 can act on: if the
# defect is fixed and observed clean, the claim has an end date. Prescriptions
# and constants are recognised so they can be surfaced without an action —
# "always do Y" is not falsified by a fix.
DEFECT_PATTERNS = [
    r"\bis broken\b", r"\bare broken\b", r"\bbreaks\b", r"\bbug\b", r"\bdefect\b",
    r"\bregression\b", r"\bfalse[- ]positives?\b", r"\bfails?\b", r"\bfailing\b",
    r"\bcrash(es|ed|ing)?\b", r"\bpanics?\b", r"\bhangs?\b", r"\bsilently\b",
    r"\bdoes ?n[o']t work\b", r"\bdoes not work\b", r"\bcannot\b", r"\bcan ?n[o']t\b",
    r"\bnever (?:fires|runs|works|arrives)\b", r"\bstale\b", r"\bloses?\b", r"\blost\b",
]
PRESCRIPTION_PATTERNS = [r"\balways\b", r"\bnever\b", r"\bmust\b", r"\bdo not\b", r"\bdon'?t\b"]
CONSTANT_PATTERNS = [r"\blimit is \d+", r"\bmax(?:imum)? (?:is|of) \d+", r"\bdefaults? to \d+"]

DEFECT_RE = re.compile("|".join(DEFECT_PATTERNS), re.IGNORECASE)
PRESCRIPTION_RE = re.compile("|".join(PRESCRIPTION_PATTERNS), re.IGNORECASE)
CONSTANT_RE = re.compile("|".join(CONSTANT_PATTERNS), re.IGNORECASE)
TASK_ID_RE = re.compile(r"\bcas-[0-9a-z]{4,}\b", re.IGNORECASE)

# Suggested op per (claim kind, verdict state). Anything not named here is
# surfaced for review with no suggested action rather than guessed at.
ACTIONS = {
    ("defect_assertion", "fixed"): "set_valid_until",
    ("defect_assertion", "recurred"): "opinion_reinforce",
    ("prescription", "recurred"): "opinion_reinforce",
}


def connect_ro(path: Path) -> sqlite3.Connection:
    conn = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    conn.execute("PRAGMA query_only=ON")
    conn.row_factory = sqlite3.Row
    return conn


def classify_claim(text: str) -> str:
    if DEFECT_RE.search(text):
        return "defect_assertion"
    if CONSTANT_RE.search(text):
        return "constant"
    if PRESCRIPTION_RE.search(text):
        return "prescription"
    return "other"


def claim_sentence(text: str, pattern: re.Pattern[str]) -> str:
    """The sentence that carries the claim — the part a reviewer must judge."""
    match = pattern.search(text)
    if not match:
        return text.strip()[:240]
    start = max(text.rfind(".", 0, match.start()), text.rfind("\n", 0, match.start())) + 1
    end = min(
        (index for index in (text.find(".", match.end()), text.find("\n", match.end())) if index != -1),
        default=len(text),
    )
    return text[start:end].strip()[:240]


STOPWORDS = {
    "after", "again", "against", "because", "before", "being", "between", "during", "from",
    "into", "over", "such", "than", "that", "them", "then", "there", "these", "this", "those",
    "through", "under", "until", "when", "where", "which", "while", "with", "without",
}


def content_words(text: str) -> set[str]:
    return {word for word in re.findall(r"[a-z][a-z-]{3,}", text.lower()) if word not in STOPWORDS}


def is_incidental(claim: str, channels: list[dict[str, str]], seed: dict[str, Any]) -> bool:
    """Is the fix merely *mentioned* near this claim rather than its subject?

    Two conditions, both measured on the live store rather than assumed:

    1. The matched token must appear in the asserting sentence at all — a
       memory that lists a task id paragraphs away is not about that defect.
    2. That sentence must also share subject matter with the fix. A note
       reading "widening CI fails the 10-minute budget (measured while landing
       cas-32ee)" satisfies (1) by parenthetical accident while making a claim
       about CI timing, not about the epic close gate; requiring two shared
       content words with the fix's title/terms separates the two cases.

    Either way the item still reaches the queue — it just never carries an
    action, because a wrong proposal is the expensive failure here.
    """
    if not any(channel["matched"].lower() in claim.lower() for channel in channels):
        return True
    subject = content_words(str(seed.get("title", "")))
    for term in seed.get("lexical_terms", []):
        subject |= content_words(str(term))
    return len(content_words(claim) & subject) < 2


def load_memories(db: Path) -> list[dict[str, Any]]:
    conn = connect_ro(db)
    rows = conn.execute(
        """
        SELECT id, COALESCE(title, '') AS title, content, COALESCE(tags, '') AS tags,
               created, importance, stability, valid_from, valid_until,
               helpful_count, harmful_count, memory_tier
          FROM entries
         WHERE archived = 0
        """
    ).fetchall()
    conn.close()
    return [dict(row) for row in rows]


def link_channels(memory: dict[str, Any], seed: dict[str, Any]) -> list[dict[str, str]]:
    """How this memory is connected to this fix — never a fuzzy score.

    Each channel names the exact token that matched, so a reviewer can reject
    the link itself rather than only the conclusion drawn from it.
    """
    haystack = f"{memory['title']}\n{memory['content']}\n{memory['tags']}"
    lowered = haystack.lower()
    channels: list[dict[str, str]] = []

    ids = {str(seed["id"]).lower()}
    base = re.match(r"(cas-[0-9a-f]{4,})", str(seed["id"]).lower())
    if base:
        ids.add(base.group(1))
    for candidate in sorted(ids):
        if candidate in lowered:
            channels.append({"channel": "task_id", "matched": candidate})

    commit = str(seed.get("fix_commit") or "")
    if len(commit) >= 7 and commit[:7].lower() in lowered:
        channels.append({"channel": "fix_commit", "matched": commit[:7]})

    for term in seed.get("lexical_terms", []):
        if term.lower() in lowered:
            channels.append({"channel": "lexical", "matched": term})

    return channels


def build_queue(
    memories: list[dict[str, Any]],
    seeds: list[dict[str, Any]],
    verdicts: dict[str, dict[str, Any]],
    verdicts_source: str,
) -> dict[str, Any]:
    seeds_by_id = {str(seed["id"]): seed for seed in seeds}
    items: list[dict[str, Any]] = []
    skipped_unobserved = 0

    for seed_id, seed in seeds_by_id.items():
        verdict = verdicts.get(seed_id)
        if not verdict:
            continue
        for memory in memories:
            channels = link_channels(memory, seed)
            if not channels:
                continue
            text = f"{memory['title']}\n{memory['content']}"
            claim_kind = classify_claim(text)
            pattern = {
                "defect_assertion": DEFECT_RE,
                "prescription": PRESCRIPTION_RE,
                "constant": CONSTANT_RE,
            }.get(claim_kind, DEFECT_RE)
            claim = claim_sentence(text, pattern)
            # A memory that merely lists a task id somewhere is not a memory
            # *about* that defect. Measured on the live store, the task-id
            # channel over-links exactly this way (a CI-timing note that
            # happens to cite three fix ids). Requiring the matched token to
            # appear in the asserting sentence keeps the link auditable and is
            # the difference between a proposal and a guess; incidental links
            # still reach the queue, but never carry an action.
            incidental = is_incidental(claim, channels, seed)
            action = None if incidental else ACTIONS.get((claim_kind, verdict["state"]))
            if verdict["state"] == "insufficient-post-fix-data":
                skipped_unobserved += 1
            items.append(
                {
                    "memory": {
                        "id": memory["id"],
                        "title": memory["title"],
                        "claim": claim,
                        "claim_kind": claim_kind,
                        "created": memory["created"],
                        "importance": memory["importance"],
                        "stability": memory["stability"],
                        "valid_from": memory["valid_from"],
                        "valid_until": memory["valid_until"],
                        "helpful_count": memory["helpful_count"],
                        "harmful_count": memory["harmful_count"],
                    },
                    "fix": {
                        "id": seed_id,
                        "title": seed["title"],
                        "fix_commit": seed.get("fix_commit"),
                        "fix_built_at": seed.get("fix_built_at"),
                    },
                    "link": channels,
                    "link_is_incidental": incidental,
                    "verdict": {
                        "state": verdict["state"],
                        "reason": verdict["reason"],
                        "source": verdicts_source,
                        "clean_post_from": verdict.get("epoch_evidence", {}).get("clean_post_from"),
                        "fix_started_running": verdict.get("epoch_evidence", {}).get("fix_started_running"),
                        "exposure": verdict.get("exposure", {}),
                    },
                    "evidence_cards": [
                        {
                            "evidence_id": card.get("evidence_id"),
                            "epoch_class": card.get("epoch_class"),
                            "channels": card.get("channels"),
                            "timestamp": card.get("timestamp"),
                            "text": str(card.get("text", ""))[:240],
                        }
                        for card in verdict.get("evidence_cards", [])
                    ],
                    "suggested_action": action,
                    "rationale": (
                        "No action proposed: the fix is only mentioned in passing, not in the "
                        "sentence that makes the claim."
                        if incidental
                        else rationale_for(claim_kind, verdict)
                    ),
                    "approved": False,
                    "approver": None,
                }
            )

    items.sort(key=lambda item: (item["suggested_action"] is None, item["memory"]["id"]))
    return {
        "note": (
            "Proposals only. Nothing here has been applied. `apply` refuses any item that is "
            "not explicitly approved by a named approver, and executes through `cas memory` so "
            "the memory system records the change itself."
        ),
        "verdicts_source": verdicts_source,
        "counts": {
            "items": len(items),
            "with_suggested_action": sum(1 for item in items if item["suggested_action"]),
            "held_for_insufficient_evidence": skipped_unobserved,
        },
        "items": items,
    }


def rationale_for(claim_kind: str, verdict: dict[str, Any]) -> str:
    state = verdict["state"]
    exposure = verdict.get("exposure", {})
    boundary = verdict.get("epoch_evidence", {}).get("clean_post_from")
    if state == "fixed" and claim_kind == "defect_assertion":
        return (
            f"The defect this memory asserts was observed clean across "
            f"{exposure.get('clean_post', 0)} post-fix observations since {boundary} "
            f"(threshold {exposure.get('threshold')}). The claim likely has an end date."
        )
    if state == "recurred":
        return (
            f"Symptom evidence matching this memory's claim occurred after the fix was "
            f"serving ({boundary}). The claim still holds."
        )
    if state == "insufficient-post-fix-data":
        return (
            f"No action proposed: {verdict['reason']}. Post-fix exposure "
            f"{exposure.get('clean_post', 0)} against threshold {exposure.get('threshold')}."
        )
    return f"No action proposed for a {claim_kind} claim under verdict {state}."


def command_for(item: dict[str, Any]) -> list[str]:
    """The `cas memory` invocation an approved item authorises. One op, one memory."""
    memory_id = item["memory"]["id"]
    action = item["suggested_action"]
    evidence = (
        f"{item['verdict']['state']} per deployed-epoch verdict for {item['fix']['id']} "
        f"({item['verdict']['source']}); clean-post from {item['verdict']['clean_post_from']}"
    )
    if action == "set_valid_until":
        return ["cas", "memory", "update", memory_id, "--valid-until", item["verdict"]["clean_post_from"]]
    if action == "opinion_reinforce":
        return ["cas", "memory", "opinion-reinforce", memory_id, "--content", evidence]
    if action == "opinion_weaken":
        return ["cas", "memory", "opinion-weaken", memory_id, "--content", evidence]
    if action == "opinion_contradict":
        return ["cas", "memory", "opinion-contradict", memory_id, "--content", evidence]
    raise ValueError(f"{memory_id}: no command for suggested action {action!r}")


def apply_queue(
    queue: dict[str, Any],
    execute: bool,
    runner: Callable[[list[str]], int] | None = None,
) -> dict[str, Any]:
    """Dry-run by default; execute only what a named approver approved.

    The two refusals below are the whole safety property: an item with no
    approval, or an approval with no approver, is never executed and is
    reported as refused rather than skipped quietly.
    """
    planned: list[dict[str, Any]] = []
    refused: list[dict[str, Any]] = []
    executed: list[dict[str, Any]] = []

    for item in queue.get("items", []):
        if not item.get("suggested_action"):
            continue
        memory_id = item["memory"]["id"]
        if not item.get("approved"):
            refused.append({"memory_id": memory_id, "reason": "not approved"})
            continue
        if not item.get("approver"):
            refused.append({"memory_id": memory_id, "reason": "approved without a named approver"})
            continue
        command = command_for(item)
        planned.append({"memory_id": memory_id, "command": command, "approver": item["approver"]})

    if execute:
        run = runner or (lambda command: subprocess.run(command, check=False).returncode)
        for plan in planned:
            code = run(plan["command"])
            executed.append({**plan, "exit_code": code})

    return {
        "executed": executed,
        "planned": planned,
        "refused": refused,
        "mode": "execute" if execute else "dry-run",
    }


def evaluate(queue: dict[str, Any], labels: dict[str, str]) -> dict[str, Any]:
    """Precision of the flags on a human-labelled sample.

    Labels map ``"<memory_id>|<fix_id>"`` to ``correct`` / ``incorrect``. Only
    items carrying a suggested action are scored: the queue's cost is a wrong
    proposal, not a missing one.
    """
    scored = correct = 0
    unlabelled: list[str] = []
    mistakes: list[dict[str, Any]] = []
    for item in queue.get("items", []):
        if not item["suggested_action"]:
            continue
        key = f"{item['memory']['id']}|{item['fix']['id']}"
        label = labels.get(key)
        if label is None:
            unlabelled.append(key)
            continue
        scored += 1
        if label == "correct":
            correct += 1
        else:
            mistakes.append({"key": key, "suggested_action": item["suggested_action"], "label": label})
    return {
        "scored": scored,
        "correct": correct,
        "precision": round(correct / scored, 4) if scored else None,
        "unlabelled": unlabelled,
        "mistakes": mistakes,
    }


def write_json(path: Path, payload: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")


def load_verdicts_from(payload: Any) -> dict[str, dict[str, Any]]:
    """Accepts M3's ``{"verdicts": [...]}`` envelope or a bare list."""
    verdicts = payload["verdicts"] if isinstance(payload, dict) else payload
    return {str(verdict["id"]): verdict for verdict in verdicts}


def load_verdicts(path: Path) -> dict[str, dict[str, Any]]:
    return load_verdicts_from(json.loads(path.read_text()))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)

    queue = sub.add_parser("queue", help="build the review queue (read-only)")
    queue.add_argument("--memories-db", type=Path, required=True)
    queue.add_argument("--seeds", type=Path, required=True)
    queue.add_argument("--verdicts", type=Path, required=True)
    queue.add_argument("--output", type=Path, required=True)

    apply_cmd = sub.add_parser("apply", help="dry-run (default) or execute approved items")
    apply_cmd.add_argument("--queue", type=Path, required=True)
    apply_cmd.add_argument("--execute", action="store_true", help="run the approved `cas memory` ops")
    apply_cmd.add_argument("--output-receipt", type=Path)

    evaluate_cmd = sub.add_parser("evaluate", help="precision of the flags on a labelled sample")
    evaluate_cmd.add_argument("--queue", type=Path, required=True)
    evaluate_cmd.add_argument("--labels", type=Path, required=True)
    evaluate_cmd.add_argument("--output", type=Path)

    args = parser.parse_args(argv)

    if args.command == "queue":
        payload = build_queue(
            load_memories(args.memories_db),
            json.loads(args.seeds.read_text()),
            load_verdicts(args.verdicts),
            str(args.verdicts),
        )
        write_json(args.output, payload)
        print(json.dumps(payload["counts"], indent=2, sort_keys=True))
        return 0

    if args.command == "apply":
        result = apply_queue(json.loads(args.queue.read_text()), args.execute)
        if args.output_receipt:
            write_json(args.output_receipt, result)
        print(json.dumps(result, indent=2, sort_keys=True))
        # A run that refused everything it was asked to do must not look like success.
        return 0 if not result["refused"] or result["planned"] else 1

    result = evaluate(json.loads(args.queue.read_text()), json.loads(args.labels.read_text()))
    if args.output:
        write_json(args.output, result)
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":  # pragma: no cover
    raise SystemExit(main())
