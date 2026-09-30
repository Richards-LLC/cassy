#!/usr/bin/env python3
"""Release review body using the same template as cas worktree pr-body.

Adapted from Matt Pocock's MIT PR skill, credited to Dex Horthy / HumanLayer
show-me. See docs/review/PR_TEMPLATE_LICENSE and docs/review/README.md.
"""
import argparse
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
TEMPLATE = ROOT / "docs/review/pr-body.md"


def render(fields):
    # One pass preserves template-like text in user evidence or CHANGELOG.
    return re.sub(r"\{\{([a-z]+)\}\}", lambda match: fields[match[1]],
                  TEMPLATE.read_text())


def changelog_section(text, version):
    lines = text.splitlines(keepends=True)
    heading = f"## [{version}]"
    start = next((i for i, line in enumerate(lines) if line.startswith(heading)), None)
    if start is None:
        raise ValueError(f"could not derive CHANGELOG section for {version}")
    end = next((i for i in range(start + 1, len(lines)) if lines[i].startswith("## [")), len(lines))
    return "".join(lines[start:end]).rstrip(), lines[end].strip() if end < len(lines) else "not supplied"


def release_body(changelog, version, gate_log=None, risk=None, door=None):
    section, previous = changelog_section(changelog, version)
    rows = [] if gate_log is None else [line for line in gate_log.splitlines()
                                       if line.startswith(("PASS", "FAIL"))]
    return render({
        "summary": f"```diff\n- {previous}\n+ release {version}\n```",
        "before": "Previous release; base-vs-change behavior run not supplied.",
        "after": "Release gate results recorded below." if rows else "Release gate evidence not supplied.",
        "evidence": "Attach the QA bundle or verify-before-claim base-vs-change run when applicable.",
        "details": f"\n### CHANGELOG\n\n{section}\n\n### Gate rows\n\n" +
                   ("```text\n" + "\n".join(rows) + "\n```\n" if rows else "Not supplied.\n"),
        "risk": risk or "not declared",
        "door": door or "not declared",
    })


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--changelog", type=Path, required=True)
    parser.add_argument("--gate-log", type=Path)
    parser.add_argument("--risk", help="Recorded release-task risk, comma-separated")
    parser.add_argument("--door", choices=("one-way", "two-way"))
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.risk:
        risks = [item.strip() for item in args.risk.split(",")]
        if (any(item not in ("none", "blast-radius", "platform", "concurrency") for item in risks)
                or len(set(risks)) != len(risks) or ("none" in risks and len(risks) > 1)):
            parser.error("invalid recorded risk")
        args.risk = ", ".join(risks)
    log = args.gate_log.read_text() if args.gate_log and args.gate_log.exists() else None
    try:
        body = release_body(args.changelog.read_text(), args.version, log, args.risk, args.door)
    except ValueError as error:
        parser.error(str(error))
    args.output.write_text(body)


if __name__ == "__main__":
    main()
