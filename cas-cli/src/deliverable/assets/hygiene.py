#!/usr/bin/env python3
"""Audience hygiene for a deliverable's visible text.

Lifted from cas-sow-delivery-report's check-sow.py hygiene. Run by
`cas deliverable finalize`:

  hygiene.py REPORT.html --audience internal|customer [--allow WORD ...]

Reads the text a reader sees (tags, scripts and styles removed; alt and title
text kept) and prints one JSON receipt whose findings name each hit with its
line. Internal output refuses credential shapes and absolute home paths.
Customer output also refuses ticket labels, issue or PR numbers, process
words, tool names, local or preview hostnames and placeholders. A staging
environment URL stays allowed: SOW reports name the environment they show. --allow
exempts a word that is the client's own (a product named "Epic").
"""
import argparse
import html as html_lib
import json
import re
from pathlib import Path

# Common client identifiers are not tracker keys. Exempt only these named
# standard/model families and two-/four-digit fiscal years, not every prefix.
CLIENT_IDENTIFIERS = re.compile(
    r"\b(?:COVID-19|SHA-(?:1|224|256|384|512)|UTF-(?:8|16|32)|"
    r"SOC-2|ASC-606|ISO-[0-9]{4,5}|GPT-[0-9]+(?:\.[0-9]+)?|"
    r"(?:Q[1-4]|H[12])-[0-9]{4}|FY-(?:[0-9]{2}|[0-9]{4}))\b", re.I)
INTERNAL = [
    ("credential", re.compile(
        r"\beyJ[\w-]{8,}\.[\w-]{8,}\.[\w-]{8,}|\bBearer\s+[\w.~+/-]{20,}|"
        r"\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|sk-[A-Za-z0-9_-]{20,}|"
        r"xox[abpr]-[A-Za-z0-9-]{10,}|AKIA[0-9A-Z]{16})\b|"
        r"\b(?:session|cookie|token|secret|password)\s*[=:]\s*[A-Za-z0-9%._~+/-]{16,}", re.I)),
    ("home path", r"(?:/home|/Users)/[A-Za-z0-9._-]+/|[A-Za-z]:\\Users\\"),
]
CUSTOMER = INTERNAL + [
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
    ("tool name", r"\bCassy\b|\bClaude\b|\bCodex\b"),
    ("local or preview URL", r"localhost|127\.0\.0\.1|\.vercel\.app"),
    ("placeholder", r"\bTODO\b|lorem"),
]
BLOCK_END = re.compile(r'</(?:p|div|li|tr|h[1-6]|figcaption|section|header|footer|dt|dd|pre|table|caption)>|<br\s*/?>', re.I)


def visible_lines(markup):
    """The text a reader sees, one line per block."""
    markup = re.sub(r'<!--.*?-->|<(script|style)\b.*?</\1>', ' ', markup, flags=re.S | re.I)
    markup = re.sub(r'<[^>]*?\b(?:alt|title)="([^"]*)"[^>]*>', lambda m: ' ' + m.group(1) + ' ', markup)
    markup = BLOCK_END.sub('\n', markup)
    text = html_lib.unescape(re.sub(r'<[^>]+>', ' ', markup))
    return [' '.join(line.split()) for line in text.splitlines()]


def scan(text, audience, allow=()):
    """[(line number, label, match, line)] for every hit in visible text."""
    allowed = [re.compile(rf"\b{re.escape(word)}\b", re.I) for word in allow]
    rules = CUSTOMER if audience == 'customer' else INTERNAL
    hits = []
    for number, line in enumerate(visible_lines(text), 1):
        if not line:
            continue
        scrubbed = CLIENT_IDENTIFIERS.sub("", line) if audience == 'customer' else line
        for pattern in allowed:
            scrubbed = pattern.sub("", scrubbed)
        for label, pattern in rules:
            match = pattern.search(scrubbed) if isinstance(pattern, re.Pattern) else re.search(pattern, scrubbed, re.I)
            if match:
                hits.append((number, label, match.group(0), line))
    return hits


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('report', type=Path)
    parser.add_argument('--audience', choices=('internal', 'customer'), required=True)
    parser.add_argument('--allow', action='append', default=[])
    args = parser.parse_args()
    hits = scan(args.report.read_text(encoding='utf-8'), args.audience, args.allow)
    findings = [{'code': 'hygiene', 'message': f'{label} {match!r} in visible line {number}: {line[:160]}'}
                for number, label, match, line in hits]
    print(json.dumps({'audience': args.audience, 'findings': findings}))


if __name__ == '__main__':
    main()
