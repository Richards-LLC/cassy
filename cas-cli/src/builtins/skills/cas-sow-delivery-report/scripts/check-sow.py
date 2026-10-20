#!/usr/bin/env python3
"""Gate checks for cas-sow-delivery-report.

Usage:
  python3 check-sow.py gap GAP-CHECK.md
  python3 check-sow.py hygiene REPORT.md [--allow WORD ...]

gap reads the first markdown table whose first column holds R1..Rn:

  | R# | The SOW says | Status | Evidence | Owner or open decision |
  | --- | --- | --- | --- | --- |
  | R1 | "A weekly dashboard for every store." | Met (live) | shot R1-dashboard | |
  | R2 | "Forecast accuracy above 90%." | Met (measured) | report of 2026-09-30 | |
  | R3 | "Managers export any table." | Partial | export works | Client: one file or one per table? |
  | R4 | "Email digest." | Gap | not built | us |
  | R5 | "Regional view." | Gap → PM | SOW does not define regions | PM |

Statuses: Met (live), Met (measured), Partial, Gap, Gap → PM. A Partial row
names the client's open decision in the last column. Exit 1 when any row is
a Gap (the SOW is not complete) or a row is malformed; exit 0 otherwise.

hygiene prints every report line that carries internal vocabulary a client
must not see and exits 1 when there is one. --allow exempts a word that is
the client's own (a product named "Epic"); record the reason in the brief.
"""
import re
import sys
from pathlib import Path

STATUSES = ("Met (live)", "Met (measured)", "Partial", "Gap", "Gap → PM")
# Common client identifiers are not tracker keys. Exempt only these named
# standard/model families and two-/four-digit fiscal years, not every prefix.
CLIENT_IDENTIFIERS = re.compile(
    r"\b(?:COVID-19|SHA-(?:1|224|256|384|512)|UTF-(?:8|16|32)|"
    r"SOC-2|ASC-606|ISO-[0-9]{4,5}|GPT-[0-9]+(?:\.[0-9]+)?|"
    r"(?:Q[1-4]|H[12])-[0-9]{4}|FY-(?:[0-9]{2}|[0-9]{4}))\b", re.I)
HYGIENE = [
    # CAS ids include all-digit and longer collision suffixes. Known PROJ/ENG
    # keys also ignore case; other numeric trackers use uppercase prefixes.
    # Lowercase years such as mid-2026 remain prose.
    # Other legacy hex keys need a digit and a hex letter, unlike self-feed.
    ("ticket label", re.compile(
        r"\b(?:(?i:cas-[0-9a-f]{4,}|(?:proj|eng)-[0-9]+)|[A-Z][A-Z0-9]+-[0-9]+|"
        r"[A-Za-z]{2,6}-(?=[0-9a-fA-F]*[0-9])(?=[0-9a-fA-F]*[a-fA-F])[0-9a-fA-F]{4,})\b")),
    ("issue or PR number", r"#[0-9]{2,5}\b"),
    ("process word", r"\bPR\b|pull request|\bbranch\b|\bcommit\b|\bagent\b|\bworker\b"
                     r"|\bsupervisor\b|\bepic\b|\bsprint\b|\bticket\b"),
    ("local or preview URL", r"localhost|127\.0\.0\.1|\.vercel\.app"),
    ("placeholder", r"\bTODO\b|lorem"),
]


def cells(line):
    return [cell.strip() for cell in line.strip().strip("|").split("|")]


def gap(path):
    rows = [cells(line) for line in Path(path).read_text(encoding="utf-8").splitlines()
            if re.match(r"^\|\s*R[0-9]+\s*\|", line)]
    if not rows:
        print(f"{path}: no R1..Rn rows; use the table format in this script's header", file=sys.stderr)
        return 1
    problems, gaps, counts = [], [], {status: 0 for status in STATUSES}
    for row in rows:
        rid = row[0]
        status = row[2].replace("->", "→") if len(row) > 2 else ""
        status = re.sub(r"\s+", " ", status.strip("*` "))
        if status not in STATUSES:
            problems.append(f"{rid}: status {status!r} is not one of {', '.join(STATUSES)}")
            continue
        counts[status] += 1
        if status.startswith("Gap"):
            gaps.append(f"{rid} ({'PM' if status == 'Gap → PM' else 'ours'})")
        if status == "Partial" and (len(row) < 5 or not row[4]):
            problems.append(f"{rid}: Partial needs the client's open decision in the last column")
    met = counts["Met (live)"] + counts["Met (measured)"]
    print(f"{met} of {len(rows)} met ({counts['Met (live)']} live, {counts['Met (measured)']} measured), "
          f"{counts['Partial']} partial, {len(gaps)} gap")
    for problem in problems:
        print(f"FAIL {problem}", file=sys.stderr)
    if gaps:
        print(f"STOP: {len(gaps)} Gap row(s): {', '.join(gaps)}. The SOW is not complete, so the "
              "completion report and the client message stop here. Finish our gaps and take the PM "
              "rows to the PM; for a status update now, produce the status variant "
              "(references/report-contract.md).", file=sys.stderr)
    return 1 if problems or gaps else 0


def hygiene(path, allow):
    allowed = [re.compile(rf"\b{re.escape(word)}\b", re.I) for word in allow]
    hits = 0
    for number, line in enumerate(Path(path).read_text(encoding="utf-8").splitlines(), 1):
        scrubbed = CLIENT_IDENTIFIERS.sub("", line)
        for pattern in allowed:
            scrubbed = pattern.sub("", scrubbed)
        for label, pattern in HYGIENE:
            match = pattern.search(scrubbed) if isinstance(pattern, re.Pattern) else re.search(pattern, scrubbed, re.I)
            if match:
                hits += 1
                print(f"{path}:{number}: {label} {match.group(0)!r}: {line.strip()}")
    if hits:
        print(f"FAIL {hits} internal-vocabulary hit(s); a client report carries only the client's "
              "and the SOW's words", file=sys.stderr)
    return 1 if hits else 0


def main(argv):
    if len(argv) >= 2 and argv[0] == "gap":
        return gap(argv[1])
    if len(argv) >= 2 and argv[0] == "hygiene":
        allow = [argv[i + 1] for i in range(2, len(argv) - 1) if argv[i] == "--allow"]
        return hygiene(argv[1], allow)
    print(__doc__.strip(), file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
