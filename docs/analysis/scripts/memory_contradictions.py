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
  existing ``mcp__cas__memory`` opinion ops, so the memory system's own audit
  trail records the outcome rather than this script writing behind its back.
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


BLOCK_SPLIT = re.compile(r"\n\s*\n")
SENTENCE_SPLIT = re.compile(r"(?<=[.!?])\s+|\n")


def blocks_containing(text: str, token: str) -> list[str]:
    """The paragraphs that mention the fix — the reviewer's reading window."""
    return [block for block in BLOCK_SPLIT.split(text) if token in block.lower()]


def sentences(block: str) -> list[str]:
    return [part.strip() for part in SENTENCE_SPLIT.split(block) if part.strip()]


STOPWORDS = {
    "after", "again", "against", "because", "before", "being", "between", "during", "from",
    "into", "over", "such", "than", "that", "them", "then", "there", "these", "this", "those",
    "through", "under", "until", "when", "where", "which", "while", "with", "without",
}


def content_words(text: str) -> set[str]:
    return {word for word in re.findall(r"[a-z][a-z-]{3,}", text.lower()) if word not in STOPWORDS}


def subject_words(seed: dict[str, Any]) -> set[str]:
    """The fix's own subject matter.

    Deliberately the *title's* words only. A symptom phrase like "delivery is
    not accounted for" is distinctive as a phrase and useless as words:
    measured on the live store, counting "delivery" as subject matter made a
    CI-timing note that mentions "a same-day delivery" look like a claim about
    the epic close gate. Phrases are matched whole, below.
    """
    words = content_words(str(seed.get("title", "")))
    for term in seed.get("structured_terms", []):
        words |= content_words(str(term).replace("-", " "))
    return words


def subject_phrases(seed: dict[str, Any]) -> list[str]:
    return [str(term).lower() for term in seed.get("lexical_terms", []) if len(str(term).split()) > 1]


def is_about(sentence: str, seed: dict[str, Any]) -> bool:
    """Is this sentence making a claim *about* the fix's subject?

    A verbatim symptom phrase settles it. Otherwise the sentence must share at
    least two of the fix title's content words — one shared word is how a note
    about something else brushes past the same topic.
    """
    lowered = sentence.lower()
    if any(phrase in lowered for phrase in subject_phrases(seed)):
        return True
    return len(content_words(sentence) & subject_words(seed)) >= 2


def claim_for(text: str, channels: list[dict[str, str]], seed: dict[str, Any]) -> tuple[str, str, bool]:
    """The claim this evidence speaks to, anchored at the link.

    Earlier this took the memory's *first* defect sentence and then asked
    whether the fix was mentioned in it. On the live store that reads the wrong
    claim: a five-instance defect-class memory whose title says "defect" gets
    judged on its title while the sentence that actually names the fix sits
    four paragraphs down. So: read the paragraphs that mention the fix, and
    take the first sentence in them that both asserts something and is about
    the fix's subject. If no such sentence exists the link is incidental — the
    item still reaches the queue, it just never carries an action, because a
    wrong proposal is the expensive failure here.
    """
    fallback: tuple[str, str] | None = None
    for channel in channels:
        token = channel["matched"].lower()
        for block in blocks_containing(text, token):
            for sentence in sentences(block):
                kind = classify_claim(sentence)
                if fallback is None and token in sentence.lower():
                    fallback = (sentence, kind)
                if kind != "other" and is_about(sentence, seed):
                    return sentence[:240], kind, False
    if fallback:
        return fallback[0][:240], fallback[1], True
    return claim_sentence(text, DEFECT_RE), classify_claim(text), True


def independent_claims(text: str) -> int:
    """How many distinct claim-bearing sentences this memory carries.

    A memory is only "the claim under review" when it makes one. CAS memories
    are session notes: 2026-08-15-12 catalogues five separate defects, and
    ending its validity because one of them shipped a fix would retire four
    claims nobody measured.
    """
    return sum(1 for sentence in sentences(text) if classify_claim(sentence) != "other")


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

    # A memory names the ticket the defect was FILED under, not the commit that
    # fixed it: on the live store, every memory asserting the epic-close
    # false-positive cites cas-b192 (the ticket) and none cites cas-32ee (the
    # fix). Without this channel the fix-side link is empty exactly for the
    # memories the queue exists to find. `defect_ids` is the seed author's
    # explicit, auditable statement of which ticket this fix closes out.
    for defect_id in seed.get("defect_ids", []):
        if str(defect_id).lower() in lowered:
            channels.append({"channel": "defect_id", "matched": str(defect_id).lower()})

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
            text = f"{memory['title']}\n\n{memory['content']}"
            # A memory that merely lists a task id somewhere is not a memory
            # *about* that defect. Measured on the live store, the task-id
            # channel over-links exactly this way (a CI-timing note that
            # happens to cite three fix ids). The claim is therefore read at
            # the link and must be about the fix's subject; incidental links
            # still reach the queue, but never carry an action.
            claim, claim_kind, incidental = claim_for(text, channels, seed)
            claims_in_memory = independent_claims(text)
            action = None if incidental else ACTIONS.get((claim_kind, verdict["state"]))
            # An end date applies to the whole memory, so it may only be
            # proposed when the whole memory is the claim that was measured.
            # A session note carrying five claims gets the weaker, additive
            # signal instead — the evidence is real, its scope is not the
            # entire note.
            if action == "set_valid_until" and claims_in_memory > 1:
                action = "opinion_weaken"
            if verdict["state"] == "insufficient-post-fix-data":
                skipped_unobserved += 1
            items.append(
                {
                    "memory": {
                        "id": memory["id"],
                        "title": memory["title"],
                        "claim": claim,
                        "claim_kind": claim_kind,
                        "independent_claims": claims_in_memory,
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
                        else rationale_for(claim_kind, verdict, action, claims_in_memory)
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


def rationale_for(claim_kind: str, verdict: dict[str, Any], action: str | None = None, claims: int = 1) -> str:
    state = verdict["state"]
    exposure = verdict.get("exposure", {})
    boundary = verdict.get("epoch_evidence", {}).get("clean_post_from")
    if state == "fixed" and claim_kind == "defect_assertion":
        observed = (
            f"The defect this memory asserts was observed clean across "
            f"{exposure.get('clean_post', 0)} post-fix observations since {boundary} "
            f"(threshold {exposure.get('threshold')})."
        )
        if action == "opinion_weaken":
            return (
                f"{observed} This memory makes {claims} separate claims, so an end date would "
                f"retire {claims - 1} the evidence says nothing about; weaken the one that was "
                f"measured instead."
            )
        return f"{observed} The claim likely has an end date."
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


MEMORY_TOOL = "mcp__cas__memory"


def operation_for(item: dict[str, Any]) -> dict[str, Any]:
    """The memory-system call an approved item authorises. One op, one memory.

    This is an ``mcp__cas__memory`` call rather than a shell command, because
    that is where the ops actually live: measured on cas 2.72.0, the `cas`
    binary exposes only `memory share` / `memory unshare` — there is no
    `cas memory update` and no `opinion-*` subcommand. Emitting a shell command
    would have produced a receipt full of invocations that exit 2, which is
    exactly the "verification machinery that is not itself verified" failure
    this queue exists to catch elsewhere.
    """
    memory_id = item["memory"]["id"]
    action = item["suggested_action"]
    evidence = (
        f"{item['verdict']['state']} per deployed-epoch verdict for {item['fix']['id']} "
        f"({item['verdict']['source']}); clean-post from {item['verdict']['clean_post_from']}"
    )
    if action == "set_valid_until":
        arguments = {"action": "update", "id": memory_id, "valid_until": item["verdict"]["clean_post_from"]}
    elif action in {"opinion_reinforce", "opinion_weaken", "opinion_contradict"}:
        arguments = {"action": action, "id": memory_id, "content": evidence}
    else:
        raise ValueError(f"{memory_id}: no memory operation for suggested action {action!r}")
    return {"tool": MEMORY_TOOL, "arguments": arguments}


def apply_queue(
    queue: dict[str, Any],
    execute: bool,
    executor: list[str] | None = None,
    runner: Callable[[list[str], str], int] | None = None,
) -> dict[str, Any]:
    """Dry-run by default; execute only what a named approver approved.

    The refusals below are the whole safety property: an item with no approval,
    or an approval with no approver, is never executed and is reported as
    refused rather than skipped quietly. ``--execute`` additionally needs an
    ``executor`` — a command that can actually reach ``mcp__cas__memory`` and
    receives one operation as JSON on stdin. Without one this reports the
    approved operations and executes nothing, rather than claiming a mutation
    it cannot perform.
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
        planned.append({"memory_id": memory_id, "operation": operation_for(item), "approver": item["approver"]})

    mode = "dry-run"
    if execute and not executor:
        mode = "no-executor"
        for plan in planned:
            refused.append({
                "memory_id": plan["memory_id"],
                "reason": f"--execute needs --executor: these ops run through {MEMORY_TOOL}, not a shell",
            })
    elif execute:
        mode = "execute"
        run = runner or (
            lambda command, payload: subprocess.run(command, input=payload, text=True, check=False).returncode
        )
        for plan in planned:
            code = run(list(executor or []), json.dumps(plan["operation"], sort_keys=True))
            executed.append({**plan, "exit_code": code})

    return {
        "executed": executed,
        "planned": planned,
        "refused": refused,
        "mode": mode,
    }


def evaluate(queue: dict[str, Any], labels: dict[str, str]) -> dict[str, Any]:
    """Precision of the flags on a human-labelled sample.

    Labels map ``"<memory_id>|<fix_id>"`` to ``correct`` / ``incorrect``.

    Two numbers, because the queue makes two kinds of mistake and only one of
    them is visible in a precision figure. ``precision`` scores the items that
    carry a proposed action — the expensive failure, a wrong mutation of a
    memory. ``decision_accuracy`` scores every linked pair including the ones
    held back, because deciding *not* to propose is also a decision, and a
    queue that stays silent about everything would otherwise score 100%.
    """
    scored = correct = 0
    decision_scored = decision_correct = 0
    unlabelled: list[str] = []
    decision_unlabelled: list[str] = []
    mistakes: list[dict[str, Any]] = []
    decision_mistakes: list[dict[str, Any]] = []
    for item in queue.get("items", []):
        key = f"{item['memory']['id']}|{item['fix']['id']}"
        label = labels.get(key)
        action = item["suggested_action"]
        if label is None:
            decision_unlabelled.append(key)
            if action:
                unlabelled.append(key)
            continue
        decision_scored += 1
        if label == "correct":
            decision_correct += 1
        else:
            decision_mistakes.append({"key": key, "suggested_action": action, "label": label})
        if not action:
            continue
        scored += 1
        if label == "correct":
            correct += 1
        else:
            mistakes.append({"key": key, "suggested_action": action, "label": label})
    return {
        "scored": scored,
        "correct": correct,
        "precision": round(correct / scored, 4) if scored else None,
        "unlabelled": unlabelled,
        "mistakes": mistakes,
        "decision_scored": decision_scored,
        "decision_correct": decision_correct,
        "decision_accuracy": round(decision_correct / decision_scored, 4) if decision_scored else None,
        "decision_unlabelled": decision_unlabelled,
        "decision_mistakes": decision_mistakes,
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
    apply_cmd.add_argument("--execute", action="store_true", help=f"run the approved {MEMORY_TOOL} ops")
    apply_cmd.add_argument(
        "--executor", nargs="+",
        help=f"command that can reach {MEMORY_TOOL}; each approved operation arrives as JSON on stdin",
    )
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
        result = apply_queue(json.loads(args.queue.read_text()), args.execute, args.executor)
        if args.output_receipt:
            write_json(args.output_receipt, result)
        print(json.dumps(result, indent=2, sort_keys=True))
        # A run that refused everything it was asked to do must not look like success.
        if result["mode"] == "no-executor":
            return 1
        return 0 if not result["refused"] or result["planned"] else 1

    result = evaluate(json.loads(args.queue.read_text()), json.loads(args.labels.read_text()))
    if args.output:
        write_json(args.output, result)
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":  # pragma: no cover
    raise SystemExit(main())
