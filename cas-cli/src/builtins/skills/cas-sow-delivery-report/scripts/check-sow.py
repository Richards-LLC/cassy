#!/usr/bin/env python3
"""Gap check for cas-sow-delivery-report.

Usage:
  python3 check-sow.py gap GAP-CHECK.md

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

Client-facing hygiene runs in `cas deliverable finalize --audience customer`.
"""
import re
import sys
from pathlib import Path

STATUSES = ("Met (live)", "Met (measured)", "Partial", "Gap", "Gap → PM")


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


def main(argv):
    if len(argv) >= 2 and argv[0] == "gap":
        return gap(argv[1])
    print(__doc__.strip(), file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
