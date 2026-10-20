#!/usr/bin/env python3
"""Lint harness-diary Slack threads against docs/RELEASE_SLACK_RUBRIC.md."""

from __future__ import annotations

import re
import sys
from pathlib import Path

THREAD_HEADINGS = ("Parent post", "Grok reply", "Claude reply", "Codex reply")
TICKET_ID = re.compile(r"\bcas-[0-9a-f]{4,}\b", re.IGNORECASE)
FENCED_TEXT = re.compile(r"```text\r?\n(.*?)\r?\n```", re.DOTALL)


def lint(path: Path) -> None:
    text = path.read_text(encoding="utf-8")
    headings = list(re.finditer(r"^## ([^\r\n]+)\s*$", text, re.MULTILINE))
    selected = [(match, match.group(1)) for match in headings
                if match.group(1) == "Parent post" or match.group(1).endswith(" reply")]
    actual = [heading for _, heading in selected]
    if actual != list(THREAD_HEADINGS):
        raise ValueError("must contain one parent and exactly three replies ordered Grok, Claude, Codex")

    for match, heading in selected:
        end = next((next_heading.start() for next_heading in headings
                    if next_heading.start() > match.start()), len(text))
        section = text[match.end():end]
        bodies = FENCED_TEXT.findall(section)
        if len(bodies) != 1 or not bodies[0].strip():
            raise ValueError(f"{heading} must contain exactly one non-empty fenced text body")
        if TICKET_ID.search(bodies[0]):
            raise ValueError(f"{heading} body contains a ticket, task, or epic ID")


def main(argv: list[str]) -> int:
    if len(argv) < 3 or argv[1] != "--lint":
        print("usage: release-diary-thread.py --lint DRAFT...", file=sys.stderr)
        return 2
    failed = False
    for name in argv[2:]:
        path = Path(name)
        try:
            lint(path)
        except (OSError, ValueError) as error:
            print(f"error: {path}: {error}", file=sys.stderr)
            failed = True
        else:
            print(f"ok: {path}: valid harness-diary thread")
    return int(failed)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
