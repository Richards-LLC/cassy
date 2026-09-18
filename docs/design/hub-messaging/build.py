#!/usr/bin/env python3
"""Generate 2026-09-18-hub-messaging-study.html from its markdown source (cas-c3c5).

Markdown is the source of truth; this script converts a constrained subset (headings,
paragraphs, lists, tables, fenced code, inline code/emphasis/links) and replaces figure
markers of the form `<!-- figure: name -->` with self-contained SVG/HTML figures and
data-URI images built from refs/ and mockups/png/. One file, no network, no build step for
the reader. Run: python3 docs/design/hub-messaging/build.py
"""
import base64, html, io, json, re, sys
sys.dont_write_bytecode = True
from pathlib import Path
from PIL import Image

HERE = Path(__file__).resolve().parent
MD = HERE / '2026-09-18-hub-messaging-study.md'
OUT = HERE / '2026-09-18-hub-messaging-study.html'
REFS = HERE / 'refs'
MOCK = HERE / 'mockups' / 'png'

def data_uri(path: Path, max_w: int = 960, quality: int = 74) -> tuple[str, int, int]:
    im = Image.open(path).convert('RGB')
    w, h = im.size
    if w > max_w:
        im = im.resize((max_w, round(h * max_w / w)), Image.LANCZOS)
    buf = io.BytesIO(); im.save(buf, 'WEBP', quality=quality, method=6)
    return 'data:image/webp;base64,' + base64.b64encode(buf.getvalue()).decode(), im.size[0], im.size[1]

def inline(text: str) -> str:
    text = html.escape(text, quote=False)
    text = re.sub(r'`([^`]+)`', r'<code>\1</code>', text)
    text = re.sub(r'\*\*([^*]+)\*\*', r'<strong>\1</strong>', text)
    text = re.sub(r'(?<![*\w])\*([^*]+)\*(?![*\w])', r'<em>\1</em>', text)
    text = re.sub(r'\[([^\]]+)\]\(([^)]+)\)', r'<a href="\2">\1</a>', text)
    return text

def convert(md: str, figures) -> str:
    out, i, lines = [], 0, md.splitlines()
    while i < len(lines):
        line = lines[i]
        m = re.match(r'<!-- figure: ([\w-]+) -->', line.strip())
        if m:
            out.append(figures[m.group(1)]()); i += 1; continue
        if line.startswith('```'):
            buf = []; i += 1
            while i < len(lines) and not lines[i].startswith('```'): buf.append(lines[i]); i += 1
            out.append('<pre><code>' + html.escape('\n'.join(buf)) + '</code></pre>'); i += 1; continue
        if line.startswith('#'):
            level = len(line) - len(line.lstrip('#')); text = line[level:].strip()
            slug = re.sub(r'[^a-z0-9]+', '-', text.lower()).strip('-')
            if level == 1: i += 1; continue  # the hero renders the document title
            out.append(f'<h{level} id="{slug}">{inline(text)}</h{level}>'); i += 1; continue
        if line.startswith('|'):
            rows = []
            while i < len(lines) and lines[i].startswith('|'): rows.append(lines[i]); i += 1
            cells = [[c.strip() for c in r.strip().strip('|').split('|')] for r in rows if not re.match(r'^\|\s*:?-', r)]
            caption = ''
            if i < len(lines) and lines[i].startswith('Table:'):
                caption = f'<caption>{inline(lines[i][6:].strip())}</caption>'; i += 1
            head = ''.join(f'<th scope="col">{inline(c)}</th>' for c in cells[0])
            body = ''.join(('<tr class="decisive">' if r[0].startswith('**') else '<tr>') + ''.join(f'<td>{inline(c)}</td>' for c in r) + '</tr>' for r in cells[1:])
            table = f'<div class="table-scroll"><table>{caption}<thead><tr>{head}</tr></thead><tbody>{body}</tbody></table></div>'
            if caption.startswith('<caption>References'): table = f'<details open><summary>The same twenty-plus references as a table</summary>{table}</details>'
            out.append(table); continue
        if re.match(r'^\s*[-*] ', line):
            items = []
            while i < len(lines) and re.match(r'^\s*[-*] ', lines[i]): items.append(re.sub(r'^\s*[-*] ', '', lines[i])); i += 1
            out.append('<ul>' + ''.join(f'<li>{inline(t)}</li>' for t in items) + '</ul>'); continue
        if re.match(r'^\s*\d+\. ', line):
            items = []
            while i < len(lines) and re.match(r'^\s*\d+\. ', lines[i]): items.append(re.sub(r'^\s*\d+\. ', '', lines[i])); i += 1
            out.append('<ol>' + ''.join(f'<li>{inline(t)}</li>' for t in items) + '</ol>'); continue
        if line.startswith('> '):
            out.append(f'<blockquote><p>{inline(line[2:])}</p></blockquote>'); i += 1; continue
        if line.strip() == '':
            i += 1; continue
        para = [line]; i += 1
        while i < len(lines) and lines[i].strip() and not re.match(r'^(#|\||```|<!--|> |\s*[-*] |\s*\d+\. )', lines[i]): para.append(lines[i]); i += 1
        out.append(f'<p>{inline(" ".join(para))}</p>')
    return '\n'.join(out)

def main(md_path: Path = MD, out_path: Path = OUT):
    sys.path.insert(0, str(HERE))
    import figures  # noqa: E402
    figures.MD = md_path
    FIGURES, CSS, JS, head_meta = figures.FIGURES, figures.CSS, figures.JS, figures.head_meta
    md = md_path.read_text(encoding='utf-8')
    count = len(figures.parse_refs_table()[1])
    words = {20: 'twenty', 21: 'twenty-one', 22: 'twenty-two', 23: 'twenty-three', 24: 'twenty-four', 25: 'twenty-five', 26: 'twenty-six', 27: 'twenty-seven', 28: 'twenty-eight', 29: 'twenty-nine', 30: 'thirty'}[count]
    md = md.replace('<!-- refcount --> real captures', f'{words.capitalize()} real captures').replace('<!-- refcount -->', words)
    title = re.search(r'^# (.+)$', md, re.M).group(1)
    body = convert(md, FIGURES)
    doc = f'<!DOCTYPE html>\n<html lang="en">\n<head>\n<meta charset="utf-8">\n<meta name="viewport" content="width=device-width, initial-scale=1">\n<title>{html.escape(title)} — 2026-09-18</title>\n{head_meta()}<style>\n{CSS}\n</style>\n</head>\n<body>\n<a class="skip" href="#main">Skip to content</a>\n{body}\n<script>\n{JS}\n</script>\n</body>\n</html>\n'
    out_path.write_text(doc, encoding='utf-8')
    print(f'wrote {out_path.name} {out_path.stat().st_size/1024:.0f} KB')

if __name__ == '__main__':
    args = sys.argv[1:]
    main(Path(args[0]) if args else MD, Path(args[1]) if len(args) > 1 else OUT)
