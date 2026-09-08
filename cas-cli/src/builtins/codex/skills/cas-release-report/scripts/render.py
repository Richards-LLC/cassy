#!/usr/bin/env python3
"""Render the release-report Markdown contract using only the Python stdlib."""
import argparse
import html
import json
import os
from pathlib import Path
import re
from urllib.parse import quote, urlsplit

REFERENCE_DIR = Path(__file__).resolve().parent.parent / 'references'
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
    pattern = r'`([^`]+)`|\[([^\]]+)\]\(([^\s)]+)\)|\*\*([^*]+)\*\*|\*([^*]+)\*'
    result, offset = [], 0
    for match in re.finditer(pattern, value):
        result.append(escape(value[offset:match.start()]))
        code, label, url, strong, em = match.groups()
        if code is not None:
            result.append('<code>' + escape(code) + '</code>')
        elif label is not None:
            result.append('<a href="' + safe_url(url) + '">' + escape(label) + '</a>')
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
            caption = ('Issue register · ' + str(len(rows[2:])) + ' verified closures'
                       if rows[0][0] == 'Issue' else 'Change map data · closed issues by surface')
            result.append('<div class="table-wrap"><table><caption>' + caption + '</caption><thead><tr>')
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


def groups(source):
    parts = re.split(r'^### (.+)\n', source, flags=re.M)
    result = [simple(parts[0])] if parts[0].strip() else []
    for k in range(1, len(parts), 2):
        items = re.split(r'^#### (.+)\n', parts[k + 1], flags=re.M)
        result.append(f'<section class="theme theme-{(k+1)//2}"><div class="theme-label"><span class="eyebrow">{(k+1)//2:02d}</span><h3>{inline(parts[k])}</h3></div><div class="changes">')
        if items[0].strip():
            result.append(simple(items[0]))
        for j in range(1, len(items), 2):
            match = re.fullmatch(r'\s*Was: (.*?)\n\nNow: (.*?)(\n\nSource:.*)?\s*', items[j + 1], re.S)
            if not match:
                raise ValueError('change needs Was/Now paragraphs: ' + items[j])
            was, now, trailing = match.groups()
            result.append('<article class="change"><h4>' + inline(items[j]) + '</h4><div class="pair"><p class="was"><span class="state">Was</span>' + inline(was.strip()) + '</p><p class="now"><span class="state">Now</span>' + inline(now.strip()) + '</p></div></article>')
            if trailing:
                result.append(simple(trailing))
        result.append('</div></section>')
    return '\n'.join(result)


def merge_tokens(base, override):
    for key, value in override.items():
        if isinstance(value, dict) and isinstance(base.get(key), dict):
            merge_tokens(base[key], value)
        else:
            base[key] = value


def token_css(project_root, token_path):
    tokens = json.loads((REFERENCE_DIR / 'default-tokens.json').read_text())
    candidates = [project_root / 'design-tokens.json', project_root / 'docs/design/design-tokens.json']
    selected = token_path or next((p for p in candidates if p.is_file()), None)
    if selected:
        merge_tokens(tokens, json.loads(selected.read_text()))
    def colors(scheme):
        values = []
        for key, token in tokens['color'][scheme].items():
            if key.startswith('$'):
                continue
            value = token['$value']
            if not re.fullmatch(r'[a-z][a-z0-9-]*', key) or not re.fullmatch(r'#[0-9a-fA-F]{6}|rgba?\([0-9.,% ]+\)', value):
                raise ValueError('color tokens must use named roles and hex or numeric rgb/rgba values')
            values.append('--' + key + ':' + value)
        return ';'.join(values)
    fonts = []
    for key in ('display', 'body', 'mono'):
        family = tokens['typography']['family'][key]['$value']
        if not isinstance(family, list) or not family or any(not re.fullmatch(r'[A-Za-z0-9 _-]+', f) for f in family):
            raise ValueError('font family tokens must be nonempty arrays of local font names')
        fonts.append('--font-' + key + ':' + ','.join('"' + f + '"' if ' ' in f else f for f in family))
    return colors('light'), colors('dark'), ';'.join(fonts)


def render(source, *, project_root=Path('.'), tokens=None, source_href='source.md',
           pdf_href='report.pdf', emphasis='', footer='Release report'):
    """Return standalone HTML; callers may write it or hand it to Chromium."""
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
    if len(rows[0]) != 3 or len(rows) < 4 or rows[-1][0] != 'Total':
        raise ValueError('change map needs three columns, surface rows and a Total row')
    map_rows, seen = [], set()
    for name, count, issues in rows[2:-1]:
        if not re.fullmatch(r'\d+', count) or not name or name in [r[0] for r in map_rows]:
            raise ValueError('map surface names must be unique and counts nonnegative integers')
        ids = re.findall(r'#([0-9]+)\b', issues)
        if len(ids) != int(count) or len(set(ids)) != len(ids) or seen.intersection(ids):
            raise ValueError('each counted closure needs one unique issue number: ' + name)
        seen.update(ids)
        map_rows.append((name, int(count), ids))
    total = sum(row[1] for row in map_rows)
    if str(total) != rows[-1][1]:
        raise ValueError('change map Total differs from the sum of its surfaces')
    # The ledger and map must account for the same issue set, once each.
    _, ledger_table, _ = split_table(sections['Fixes ledger'])
    ledger_rows = table_rows(ledger_table)[2:]
    ledger_ids = [re.findall(r'#([0-9]+)\b', row[0]) for row in ledger_rows]
    if any(len(ids) != 1 for ids in ledger_ids) or sorted(ids[0] for ids in ledger_ids) != sorted(seen):
        raise ValueError('fixes ledger must contain exactly the issues counted in the map')
    maximum = max(row[1] for row in map_rows)
    svg, description = [], []
    y = 46
    # Wrap long stitches into equal-area rows; never shrink marks to imply impact.
    for name, count, ids in map_rows:
        decisive = count == maximum and count > 0
        svg.append('<g data-surface="' + escape(name) + '" data-count="' + str(count) + '" data-issues="' + escape(json.dumps(ids)) + '" data-emphasis="' + str(decisive).lower() + '">')
        svg.append(f'<text x="0" y="{y+5}" class="svg-label">{escape(name)}</text>')
        for j, issue in enumerate(ids):
            svg.append(f'<circle cx="{140+(j%7)*25}" cy="{y+(j//7)*20}" r="7" class="{"decisive" if decisive else "dot"}" data-issue="{issue}"><title>Issue #{issue}</title></circle>')
        if count == 0:
            svg.append(f'<text x="134" y="{y+4}" class="svg-note">no listed issues</text>')
        svg.append(f'<text x="337" y="{y+5}" text-anchor="end" class="svg-count">{count}</text></g>')
        description.append(name + ' ' + str(count))
        y += 40 + max(0, (count-1)//7)*20
    leading, glance_table, method = split_table(sections['Release at a glance'])
    stats = []
    for row in table_rows(glance_table)[2:]:
        if len(row) != 3:
            raise ValueError('glance table needs measure, value and definition')
        label, value, definition = row
        stats.append('<div class="stat"><span class="stat-value">' + inline(value) + '</span><span class="stat-label">' + inline(label) + '</span><p>' + inline(definition) + '</p></div>')
    light, dark, fonts = token_css(Path(project_root), Path(tokens) if tokens else None)
    rendered_verdict = inline(verdict)
    if emphasis:
        if emphasis not in verdict:
            raise ValueError('emphasis must be an exact substring of the verdict')
        rendered_verdict = rendered_verdict.replace(escape(emphasis), '<em>' + escape(emphasis) + '</em>', 1)
    values = dict(TITLE=inline(title), DATE=inline(date), VERDICT=rendered_verdict,
                  LIGHT_TOKENS=light, DARK_TOKENS=dark, PRINT_TOKENS=light, FONT_TOKENS=fonts,
                  MAP_HEIGHT=y, MAP_TOTAL=total, MAP_CLAIM=inline(claim),
                  MAP_DESCRIPTION=escape(', '.join(description) + '. Each filled circle represents one closed issue. Total ' + str(total) + '.'),
                  MAP_ROWS='\n'.join(svg), MAP_LEGEND='One dot, one closed issue. ' + str(sum(r[1] > 0 for r in map_rows)) + ' issue-bearing surfaces.',
                  STATS='\n'.join(stats), METHOD=simple(leading + '\n\n' + method),
                  MAP_DATA=simple(guide) + simple(map_table) + simple(after),
                  USERS=groups(sections['What you can do now']), DEV=groups(sections['Under the hood']),
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
    parser.add_argument('--tokens', type=Path)
    parser.add_argument('--pdf-href')
    parser.add_argument('--emphasis', default='')
    parser.add_argument('--footer', default='Release report')
    args = parser.parse_args()
    try:
        output = render(args.source.read_text(encoding='utf-8'), project_root=args.project_root,
                        tokens=args.tokens, source_href=quote(os.path.relpath(args.source.resolve(), args.output.resolve().parent)),
                        pdf_href=args.pdf_href or quote(args.output.with_suffix('.pdf').name),
                        emphasis=args.emphasis, footer=args.footer)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(output, encoding='utf-8')
    except (ValueError, KeyError, OSError, TypeError) as error:
        parser.exit(1, 'release report: ' + str(error) + '\n')


if __name__ == '__main__':
    main()
