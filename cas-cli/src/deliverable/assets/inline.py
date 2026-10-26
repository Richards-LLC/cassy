#!/usr/bin/env python3
"""Make a deliverable HTML single-file by inlining its local images, within budget.

Lifted from cas-sow-delivery-report's inline-images.py. Run by
`cas deliverable finalize`:

  inline.py DRAFT.html OUT.html --base DIR --max-image-bytes N --max-images-bytes N --max-file-bytes N

Every <img src="relative/path"> and CSS url(relative/path) becomes a data URI;
paths resolve against --base. Remote references are refused, because the
deliverable must render offline. Images already inlined count too.

Images are budgeted in units: the images of one before/after pair (the nearest
element with class `captures` or `sides`), else of one <figure>, else one
<img>; the brand mark (`img.mark`) is not budgeted. A unit holding an image
over --max-image-bytes is dropped; then, while
all kept images exceed --max-images-bytes, the lowest-priority unit is dropped
(`data-priority="N"` on the img or its unit, default 0; later units first on a
tie). A dropped unit is replaced by a short note, never by one side of a pair,
and is named in the receipt. Prints one JSON receipt; a refused or missing
reference or a file over --max-file-bytes is a finding.
"""
import argparse
import base64
from html.parser import HTMLParser
import json
import mimetypes
import re
from pathlib import Path

MIME = {".jpg": "image/jpeg", ".jpeg": "image/jpeg", ".png": "image/png",
        ".webp": "image/webp", ".svg": "image/svg+xml", ".woff2": "font/woff2", ".woff": "font/woff"}
PAIR_CLASSES = {'captures', 'sides'}
VOID = {'area', 'base', 'br', 'col', 'embed', 'hr', 'img', 'input', 'link', 'meta', 'source', 'track', 'wbr'}
DROPPED_NOTE = '<div class="missing">Screenshots left out to keep this report within its size budget.</div>'


class Elements(HTMLParser):
    """Spans of <img> tags and of the elements that can hold an image unit."""

    def __init__(self, html):
        super().__init__(convert_charrefs=True)
        self.source, self.stack, self.images, self.spans = html, [], [], {}
        # HTMLParser counts lines by "\n" only; index line starts the same way.
        self.lines = [0] + [index + 1 for index, char in enumerate(html) if char == '\n']
        self.feed(html)
        self.close()

    def position(self):
        line, column = self.getpos()
        return self.lines[line - 1] + column

    def handle_starttag(self, tag, attrs):
        start = self.position()
        end = start + len(self.get_starttag_text() or '')
        attrs = dict(attrs)
        if tag == 'img' and 'mark' in (attrs.get('class') or '').split():
            return  # the brand mark is part of the brand, not a budgeted image
        if tag == 'img':
            self.images.append({'start': start, 'end': end, 'src': attrs.get('src') or '',
                                'priority': attrs.get('data-priority'), 'ancestors': list(self.stack)})
        elif tag not in VOID:
            classes = set((attrs.get('class') or '').split())
            self.stack.append({'tag': tag, 'start': start, 'classes': classes,
                               'priority': attrs.get('data-priority'), 'id': len(self.spans)})
            self.spans[len(self.spans)] = self.stack[-1]

    def handle_endtag(self, tag):
        for index in range(len(self.stack) - 1, -1, -1):
            if self.stack[index]['tag'] == tag:
                start = self.position()
                end = self.source.find('>', start) + 1
                for element in self.stack[index:]:
                    element.setdefault('end', end)
                del self.stack[index:]
                return


def priority(*values):
    for value in values:
        if value is not None and re.fullmatch(r'-?[0-9]+', str(value).strip()):
            return int(value)
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('draft', type=Path)
    parser.add_argument('out', type=Path)
    parser.add_argument('--base', type=Path, required=True)
    parser.add_argument('--max-image-bytes', type=int, required=True)
    parser.add_argument('--max-images-bytes', type=int, required=True)
    parser.add_argument('--max-file-bytes', type=int, required=True)
    args = parser.parse_args()
    base = args.base.resolve()
    html = args.draft.read_text(encoding='utf-8')
    findings, warnings = [], []

    def measure(ref):
        """(bytes, data URI or None) for one reference; None bytes when refused."""
        if re.match(r'^(https?:)?//', ref):
            findings.append({'code': 'remote-reference', 'message': f'remote reference refused: {ref}'})
            return None, None
        if ref.startswith('data:'):
            payload = ref.split(',', 1)[-1]
            return (len(payload) * 3 // 4 if ';base64' in ref.split(',', 1)[0] else len(payload)), ref
        if ref.startswith('#') or not ref:
            return 0, ref
        path = (base / ref).resolve()
        if not path.is_file():
            findings.append({'code': 'missing-file', 'message': f'missing file: {ref}'})
            return None, None
        data = path.read_bytes()
        mime = MIME.get(path.suffix.lower()) or mimetypes.guess_type(path.name)[0] or 'application/octet-stream'
        return len(data), f'data:{mime};base64,{base64.b64encode(data).decode("ascii")}'

    elements = Elements(html)
    units = {}
    for image in elements.images:
        ancestors = image['ancestors']
        holder = next((a for a in reversed(ancestors) if a['classes'] & PAIR_CLASSES), None) \
            or next((a for a in reversed(ancestors) if a['tag'] == 'figure'), None)
        key = ('element', holder['id']) if holder else ('img', image['start'])
        unit = units.setdefault(key, {'start': holder['start'] if holder else image['start'],
                                      'end': holder.get('end', image['end']) if holder else image['end'],
                                      'priority': priority(holder['priority']) if holder else 0,
                                      'images': []})
        image['bytes'], image['uri'] = measure(image['src'])
        unit['priority'] = max(unit['priority'], priority(image['priority'], unit['priority']))
        unit['images'].append(image)

    ordered = sorted(units.values(), key=lambda unit: unit['start'])
    dropped = []
    for unit in ordered:
        big = [i for i in unit['images'] if (i['bytes'] or 0) > args.max_image_bytes]
        if big:
            unit['dropped'] = (f'image over {args.max_image_bytes // 1024} KB: '
                               + ', '.join(f"{i['src'][:80]} ({i['bytes'] // 1024} KB)" for i in big))
    kept = [unit for unit in ordered if 'dropped' not in unit]
    total = sum(i['bytes'] or 0 for unit in kept for i in unit['images'])
    for unit in sorted(kept, key=lambda unit: (unit['priority'], -unit['start'])):
        if total <= args.max_images_bytes:
            break
        unit['dropped'] = f'over the {args.max_images_bytes // 1024} KB image budget for this report'
        total -= sum(i['bytes'] or 0 for i in unit['images'])

    pieces, cursor, inlined = [], 0, []
    for unit in ordered:
        if unit['start'] < cursor:  # a unit nested in a dropped one is gone with it
            continue
        pieces.append(html[cursor:unit['start']])
        if 'dropped' in unit:
            dropped.append({'refs': [i['src'][:120] for i in unit['images']],
                            'bytes': sum(i['bytes'] or 0 for i in unit['images']), 'reason': unit['dropped']})
            pieces.append(DROPPED_NOTE)
            cursor = unit['end']
            continue
        segment, offset = [], unit['start']
        for image in unit['images']:
            segment.append(html[offset:image['start']])
            tag = html[image['start']:image['end']]
            if image['uri'] and image['uri'] != image['src']:
                tag = tag.replace(image['src'], image['uri'], 1)
                inlined.append({'ref': image['src'][:120], 'bytes': image['bytes']})
            segment.append(tag)
            offset = image['end']
        segment.append(html[offset:unit['end']])
        pieces.append(''.join(segment))
        cursor = unit['end']
    pieces.append(html[cursor:])
    html = ''.join(pieces)

    def css_url(match):
        uri = measure(match.group(2))[1]
        if uri and uri != match.group(2):
            inlined.append({'ref': match.group(2)[:120], 'bytes': len(uri) * 3 // 4})
        return f"url({match.group(1)}{uri or match.group(2)}{match.group(1)})"

    html = re.sub(r'url\((["\']?)(?!data:)([^)"\']+)\1\)', css_url, html)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(html, encoding='utf-8')
    size = args.out.stat().st_size
    if size > args.max_file_bytes:
        findings.append({'code': 'file-over-budget',
                         'message': f'file is {size / 1048576:.1f} MB, over the {args.max_file_bytes / 1048576:.0f} MB budget'})
    for unit in dropped:
        warnings.append(f"dropped {', '.join(unit['refs'])}: {unit['reason']}")
    print(json.dumps({'bytes': size, 'image_bytes': total, 'inlined': inlined, 'dropped': dropped,
                      'warnings': warnings, 'findings': findings}))


if __name__ == '__main__':
    main()
