#!/usr/bin/env python3
"""Render the release-report Markdown contract using only the Python stdlib."""
import argparse
import base64
import html
import json
import os
from pathlib import Path
import re
import sys
from urllib.parse import quote, urlsplit

REFERENCE_DIR = Path(__file__).resolve().parent.parent / 'references'
# Eight equal dots fit between the area label (x < 150) and a two-digit count
# (right-aligned at 337): the last dot's edge is 160 + 7*20 + 7 = 307.
DOTS_PER_ROW = 8
SECTION_NAMES = ('Change map', 'Release at a glance', 'What you can do now',
                 'Under the hood', 'Fixes ledger', 'Install', 'Evidence and scope')


def escape(value):
    return html.escape(str(value), quote=True)


def safe_url(value):
    if any(ord(c) < 32 for c in value) or value.startswith('//'):
        raise ValueError('invalid link destination')
    if urlsplit(value).scheme.lower() not in ('', 'http', 'https', 'mailto'):
        raise ValueError('unsupported link scheme')
    return escape(value)


def inline(value):
    """Small explicit Markdown subset; raw HTML is always text."""
    pattern = (r'`([^`]+)`|\[([^\]]+)\]\(([^\s)]+)\)|<(https?://[^\s<>]+)>'
               r'|\*\*([^*]+)\*\*|\*([^*]+)\*')
    result, offset = [], 0
    for match in re.finditer(pattern, value):
        result.append(escape(value[offset:match.start()]))
        code, label, url, autolink, strong, em = match.groups()
        if code is not None:
            result.append('<code>' + escape(code) + '</code>')
        elif label is not None:
            result.append('<a href="' + safe_url(url) + '">' + escape(label) + '</a>')
        elif autolink is not None:
            result.append('<a href="' + safe_url(autolink) + '">' + escape(autolink) + '</a>')
        elif strong is not None:
            result.append('<strong>' + escape(strong) + '</strong>')
        else:
            result.append('<em>' + escape(em) + '</em>')
        offset = match.end()
    return ''.join(result) + escape(value[offset:])


def table_rows(block):
    rows = [[c.strip() for c in line.strip().strip('|').split('|')]
            for line in block.strip().splitlines()]
    if len(rows) < 2 or not all(re.fullmatch(r':?-{3,}:?', c) for c in rows[1]):
        raise ValueError('table needs a Markdown separator row')
    if any(len(row) != len(rows[0]) for row in rows):
        raise ValueError('table rows must have equal column counts; escape pipes as &#124;')
    return rows


def split_table(source):
    match = re.search(r'^\|.*(?:\n\|.*)*', source, re.M)
    if not match:
        raise ValueError('section needs a table')
    return source[:match.start()].strip(), match.group(), source[match.end():].strip()


def simple(source):
    lines, result, i = source.strip().splitlines(), [], 0
    while i < len(lines):
        line = lines[i]
        if not line.strip():
            i += 1
            continue
        if line.startswith('```'):
            code = []
            i += 1
            while i < len(lines) and not lines[i].startswith('```'):
                code.append(lines[i])
                i += 1
            if i == len(lines):
                raise ValueError('unclosed code fence')
            result.append('<pre><code>' + escape('\n'.join(code)) + '</code></pre>')
            i += 1
        elif line.startswith('|'):
            block = []
            while i < len(lines) and lines[i].startswith('|'):
                block.append(lines[i])
                i += 1
            rows = table_rows('\n'.join(block))
            if rows[0][0] == 'Issue':
                caption = 'GitHub issues closed with this release · ' + str(len(rows[2:]))
            elif rows[0][0] == 'Area' and rows[0][1] == 'Changes':
                caption = ''  # the open <summary> above names this table
            elif rows[0][0] == 'Surface':
                caption = 'Issues closed, by area'
            elif rows[0][0] == 'Asset':
                caption = 'Release assets'
            else:
                caption = ''
            result.append('<div class="table-wrap"><table>' + ('<caption>' + caption + '</caption>' if caption else '') + '<thead><tr>')
            for k, cell in enumerate(rows[0]):
                num = ' class="num"' if rows[1][k].endswith(':') else ''
                result.append('<th scope="col"' + num + '>' + inline(cell) + '</th>')
            result.append('</tr></thead><tbody>')
            for row in rows[2:]:
                result.append('<tr>')
                for k, cell in enumerate(row):
                    tag = 'th' if k == 0 else 'td'
                    attrs = ' scope="row"' if k == 0 else ''
                    attrs += ' class="num"' if rows[1][k].endswith(':') else ''
                    result.append('<' + tag + attrs + '>' + inline(cell) + '</' + tag + '>')
                result.append('</tr>')
            result.append('</tbody></table></div>')
        elif line.startswith('### '):
            result.append('<h3>' + inline(line[4:]) + '</h3>')
            i += 1
        elif line.startswith('#'):
            raise ValueError('unsupported heading: ' + line)
        elif line.startswith('- '):
            result.append('<ul>')
            while i < len(lines) and lines[i].startswith('- '):
                result.append('<li>' + inline(lines[i][2:]) + '</li>')
                i += 1
            result.append('</ul>')
        else:
            para = [line]
            i += 1
            while i < len(lines) and lines[i].strip() and not lines[i].startswith(('#', '|', '```', '- ')):
                para.append(lines[i])
                i += 1
            result.append('<p>' + inline(' '.join(para)) + '</p>')
    return '\n'.join(result)


CAPTURE_BYTES = 400_000  # per image, the same cap as an inlined font face
CAPTURE_PAIRS = 6        # per report: one representative pair per change
IMAGE_TYPES = ((b'\x89PNG\r\n\x1a\n', 'image/png'), (b'\xff\xd8\xff', 'image/jpeg'), (b'RIFF', 'image/webp'))


def warn(message):
    print('release report: warning: ' + message, file=sys.stderr)


class Captures:
    """Before | After screenshot pairs for Was -> Now cards, inlined as data URIs.

    A pair renders only when both real captures exist, are PNG, JPEG or WebP and
    fit the budget; otherwise the change renders without captures and a warning
    names why. Paths are absolute or relative to the project root.
    """

    def __init__(self, project_root):
        self.root, self.pairs = Path(project_root), 0

    def image(self, path):
        file = Path(path) if Path(path).is_absolute() else self.root / path
        if not file.is_file():
            raise ValueError('capture not found: ' + path)
        data = file.read_bytes()
        if len(data) > CAPTURE_BYTES:
            raise ValueError(f'capture over {CAPTURE_BYTES // 1000} kB: ' + path)
        kind = next((name for magic, name in IMAGE_TYPES if data.startswith(magic)), None)
        if not kind or kind == 'image/webp' and data[8:12] != b'WEBP':
            raise ValueError('capture is not a PNG, JPEG or WebP image: ' + path)
        return 'data:' + kind + ';base64,' + base64.b64encode(data).decode()

    def pair(self, title, before, after):
        if not before and not after:
            return ''
        if not (before and after):
            warn('change "' + title + '" needs both Before: and After: captures; showing none')
            return ''
        if self.pairs >= CAPTURE_PAIRS:
            warn(f'more than {CAPTURE_PAIRS} capture pairs; showing none for "' + title + '"')
            return ''
        try:
            sides = [(label, self.image(path)) for label, path in (('Before', before), ('After', after))]
        except ValueError as error:
            warn(str(error) + '; showing no captures for "' + title + '"')
            return ''
        self.pairs += 1
        return '<figure class="captures">' + ''.join(
            f'<div class="capture capture-{label.lower()}"><img src="{uri}" alt="{label}: {escape(title)}" decoding="async">'
            f'<figcaption><span class="state">{label}</span></figcaption></div>' for label, uri in sides) + '</figure>'


def groups(source, captures=None):
    captures = captures or Captures('.')
    parts = re.split(r'^### (.+)\n', source, flags=re.M)
    result = [simple(parts[0])] if parts[0].strip() else []
    for k in range(1, len(parts), 2):
        items = re.split(r'^#### (.+)\n', parts[k + 1], flags=re.M)
        result.append(f'<section class="theme theme-{(k+1)//2}"><div class="theme-label"><span class="eyebrow">{(k+1)//2:02d}</span><h3>{inline(parts[k])}</h3></div><div class="changes">')
        if items[0].strip():
            result.append(simple(items[0]))
        for j in range(1, len(items), 2):
            match = re.fullmatch(r'\s*Was: (.*?)\n\nNow: (.*?)(?:\n\nBefore: (\S+))?(?:\n\nAfter: (\S+))?(\n\nSource:.*)?\s*', items[j + 1], re.S)
            if not match:
                raise ValueError('change needs Was/Now paragraphs: ' + items[j])
            was, now, before, after, trailing = match.groups()
            result.append('<article class="change"><h4>' + inline(items[j]) + '</h4><div class="pair"><p class="was"><span class="state">Was</span>' + inline(was.strip()) + '</p><p class="now"><span class="state">Now</span>' + inline(now.strip()) + '</p></div>' + captures.pair(items[j].strip(), before, after) + '</article>')
            if trailing:
                result.append(simple(trailing))
        result.append('</div></section>')
    return '\n'.join(result)


def render(source, *, project_root=Path('.'), source_href='source.md',
           pdf_href='report.pdf', emphasis='', footer='Release report'):
    """Return HTML whose brand markers `cas deliverable` fills; this script
    holds no brand code. `cas release report` renders, fills and writes it."""
    # The CLI assembler records machine-readable release metadata in a small
    # YAML front matter block.  Keep the renderer's fixed Markdown contract
    # unchanged after that block so hand-authored exemplar sources remain
    # valid too.
    if source.startswith('---\n'):
        closing = source.find('\n---\n', 4)
        if closing < 0:
            raise ValueError('front matter needs a closing --- line')
        source = source[closing + len('\n---\n'):]
    parts = re.split(r'^## (.+)\n', source.replace('\r\n', '\n'), flags=re.M)
    names = parts[1::2]
    if tuple(names) != SECTION_NAMES:
        raise ValueError('expected sections in order: ' + ', '.join(SECTION_NAMES))
    sections = dict(zip(names, parts[2::2]))
    head = parts[0].strip().split('\n\n')
    if len(head) != 3 or not head[0].startswith('# '):
        raise ValueError('preamble needs H1 product/version, publication line, verdict paragraph')
    title, date, verdict = head[0][2:], head[1], head[2]
    before, map_table, after = split_table(sections['Change map'])
    intro = before.split('\n\n', 1)
    if len(intro) != 2 or not after:
        raise ValueError('change map needs claim, reading guide and source after table')
    claim, guide = intro
    rows = table_rows(map_table)
    header = [cell.lower() for cell in rows[0]]
    if header == ['area', 'changes', 'what changed']:
        by_change = True
    elif header == ['surface', 'issues closed', 'issue numbers']:
        by_change = False  # v1 sources: one dot per closed issue
    else:
        raise ValueError('change map header must be Area | Changes | What changed')
    if len(rows) < 3 or rows[-1][0] != 'Total':
        raise ValueError('change map needs area rows and a final Total row')
    map_rows, seen = [], set()
    for name, count, items in rows[2:-1]:
        if not re.fullmatch(r'\d+', count) or not name or name in [r[0] for r in map_rows]:
            raise ValueError('map area names must be unique and counts nonnegative integers')
        if by_change:
            ids = [item.strip() for item in items.split(';') if item.strip()]
            if int(count) == 0 or len(ids) != int(count):
                raise ValueError('each area needs one or more changes, separated by semicolons, '
                                 'matching its count: ' + name)
        else:
            ids = re.findall(r'#([0-9]+)\b', items)
            if len(ids) != int(count) or len(set(ids)) != len(ids) or seen.intersection(ids):
                raise ValueError('each counted closure needs one unique issue number: ' + name)
            seen.update(ids)
        map_rows.append((name, int(count), ids))
    total = sum(row[1] for row in map_rows)
    if str(total) != rows[-1][1]:
        raise ValueError('change map Total differs from the sum of its areas')
    _, ledger_table, _ = split_table(sections['Fixes ledger'])
    ledger_rows = table_rows(ledger_table)[2:]
    ledger_ids = [re.findall(r'#([0-9]+)\b', row[0]) for row in ledger_rows]
    if any(len(ids) != 1 for ids in ledger_ids) or len({ids[0] for ids in ledger_ids}) != len(ledger_ids):
        raise ValueError('fixes ledger needs one unique linked issue number per row')
    if not by_change and sorted(ids[0] for ids in ledger_ids) != sorted(seen):
        raise ValueError('fixes ledger must contain exactly the issues counted in the map')
    issues_closed = len(ledger_rows)
    # Page one never shows an empty row: areas without marks stay in the data table only.
    shown = [row for row in map_rows if row[1] > 0]
    maximum = max((row[1] for row in shown), default=0)
    svg, description = [], []
    y = 46
    # Wrap long stitches into equal-area rows; never shrink marks to imply impact.
    for name, count, ids in shown:
        decisive = count == maximum
        if by_change:
            svg.append('<g data-area="' + escape(name) + '" data-count="' + str(count) + '" data-changes="' + escape(json.dumps(ids)) + '" data-emphasis="' + str(decisive).lower() + '">')
        else:
            svg.append('<g data-surface="' + escape(name) + '" data-count="' + str(count) + '" data-issues="' + escape(json.dumps(ids)) + '" data-emphasis="' + str(decisive).lower() + '">')
        svg.append(f'<text x="0" y="{y+5}" class="svg-label">{escape(name)}</text>')
        for j, item in enumerate(ids):
            mark = f'data-change="{j+1}"><title>{escape(item)}</title>' if by_change else f'data-issue="{item}"><title>Issue #{item}</title>'
            svg.append(f'<circle cx="{160+(j%DOTS_PER_ROW)*20}" cy="{y+(j//DOTS_PER_ROW)*20}" r="7" class="{"decisive" if decisive else "dot"}" {mark}</circle>')
        svg.append(f'<text x="337" y="{y+5}" text-anchor="end" class="svg-count">{count}</text></g>')
        description.append(name + ' ' + str(count))
        y += 34 + max(0, (count-1)//DOTS_PER_ROW)*20
    if not shown:
        svg.append(f'<text x="0" y="{y+5}" class="svg-label">{"No changes listed" if by_change else "No GitHub issues closed"}</text>')
        y += 34
    unit = 'shipped change' if by_change else 'closed GitHub issue'
    if by_change:
        legend = ('One dot, one shipped change. ' +
                  (f'{issues_closed} GitHub issue{"" if issues_closed == 1 else "s"} closed with this release, listed in the fixes ledger.'
                   if issues_closed else "No GitHub issues are counted in this report's fixes ledger."))
    else:
        legend = 'One dot, one closed GitHub issue.'
    leading, glance_table, method = split_table(sections['Release at a glance'])
    stats = []
    for row in table_rows(glance_table)[2:]:
        if len(row) != 3:
            raise ValueError('glance table needs measure, value and definition')
        label, value, definition = row
        stats.append('<div class="stat"><span class="stat-value">' + inline(value) + '</span><span class="stat-label">' + inline(label) + '</span><p>' + inline(definition) + '</p></div>')
    captures = Captures(project_root)
    rendered_verdict = inline(verdict)
    if emphasis:
        if emphasis not in verdict:
            raise ValueError('emphasis must be an exact substring of the verdict')
        rendered_verdict = rendered_verdict.replace(escape(emphasis), '<em>' + escape(emphasis) + '</em>', 1)
    values = dict(TITLE=inline(title), DATE=inline(date), VERDICT=rendered_verdict,
                  MAP_HEIGHT=y, MAP_TOTAL=total, MAP_CLAIM=inline(claim),
                  MAP_DESCRIPTION=escape(', '.join(description) + '. Each filled circle represents one ' + unit + '. Total ' + str(total) + '.'),
                  MAP_ROWS='\n'.join(svg), MAP_LEGEND=escape(legend), MAP_UNIT=escape('CHANGES' if by_change else 'ISSUES CLOSED'),
                  MAP_SUMMARY=escape('What shipped, by area' if by_change else 'Issues closed, by area'),
                  MAP_LINK=escape('See every change' if by_change else 'Read the map data'),
                  STATS='\n'.join(stats), METHOD=simple(leading + '\n\n' + method),
                  MAP_DATA=simple(guide) + simple(map_table) + simple(after),
                  USERS=groups(sections['What you can do now'], captures), DEV=groups(sections['Under the hood'], captures),
                  LEDGER=simple(sections['Fixes ledger']), INSTALL=simple(sections['Install']),
                  PROVENANCE=simple(sections['Evidence and scope']),
                  STITCH=''.join(f'<circle cx="{8+j*23}" cy="8" r="5" fill="currentColor"/>' for j in range(6)),
                  FOOTER=inline(footer), SOURCE_HREF=safe_url(source_href), PDF_HREF=safe_url(pdf_href))
    template = (REFERENCE_DIR / 'template.html').read_text()
    return re.sub(r'\{\{([A-Z_]+)\}\}', lambda m: str(values[m[1]]), template)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--project-root', default=Path('.'), type=Path)
    parser.add_argument('--pdf-href')
    parser.add_argument('--source-href')
    parser.add_argument('--emphasis', default='')
    parser.add_argument('--footer', default='Release report')
    args = parser.parse_args()
    try:
        output = render(args.source.read_text(encoding='utf-8'), project_root=args.project_root,
                        source_href=args.source_href or quote(os.path.relpath(args.source.resolve(), args.output.resolve().parent)),
                        pdf_href=args.pdf_href or quote(args.output.with_suffix('.pdf').name),
                        emphasis=args.emphasis, footer=args.footer)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(output, encoding='utf-8')
    except (ValueError, KeyError, OSError, TypeError) as error:
        parser.exit(1, 'release report: ' + str(error) + '\n')


if __name__ == '__main__':
    main()
