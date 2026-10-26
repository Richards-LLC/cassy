#!/usr/bin/env python3
"""Resolve a project's brand for a Cassy deliverable: tokens, type and mark.

`cas deliverable brand` materializes this script beside the bundled default
tokens and runs it; it prints one JSON document {receipt, css, mark_uri}.
Brand code lives only here: renderers emit the `/*cas:brand*/` style marker
and `<!--cas:brand-mark-->` lockup marker, and the CLI fills them.

Order, after the DESIGN.md-first rule: an explicit --tokens JSON; the DESIGN.md
named by docs/deliverables/PROFILE.md; the DESIGN.md nearest --surface; the
root DESIGN.md; a single nested DESIGN.md such as apps/frontend/DESIGN.md.
`inherits:` naming another DESIGN.md is followed. Within that chain the CSS
`source` + `maps` path wins; refused or absent CSS falls to the design-spec
value keys (colors, typography, rounded). Without a DESIGN.md brand, an
internal deliverable takes design-tokens.json, then the bundled defaults; a
customer deliverable refuses both and reports a finding instead.
"""
import argparse
import base64
import json
import os
from pathlib import Path
import re
import subprocess
import sys

FACE_BYTES = 400_000
MARK_TYPES = {'.svg': 'image/svg+xml', '.png': 'image/png', '.jpg': 'image/jpeg',
              '.jpeg': 'image/jpeg', '.webp': 'image/webp'}
GENERIC_FAMILIES = {'serif', 'sans-serif', 'monospace', 'cursive', 'fantasy', 'system-ui',
                    'ui-serif', 'ui-sans-serif', 'ui-monospace', 'ui-rounded', 'math'}
FALLBACK_STACK = {'display': ['ui-sans-serif', 'system-ui', 'sans-serif'],
                  'body': ['ui-sans-serif', 'system-ui', 'sans-serif'],
                  'mono': ['ui-monospace', 'SFMono-Regular', 'Menlo', 'Consolas', 'monospace']}
# design-spec role names a project may use where the report templates read
# Petrastella roles; each missing report role takes the first project role named.
ROLE_ALIASES = {
    'ink': ('text',), 'ink-muted': ('text-muted', 'ink-secondary'), 'line': ('border',),
    'line-strong': ('border-strong', 'line', 'border'), 'good': ('success',),
    'verdict': ('accent', 'primary'), 'action': ('primary', 'link'),
    'surface-hero': ('surface-raised', 'surface'), 'verdict-soft': ('surface-sunk', 'hover', 'surface'),
    'evidence': ('ink-muted', 'text-muted'), 'danger': ('error',),
}
TYPE_ROLES = {
    'display': ('display', 'brand', 'title', 'headline', 'h1', 'page-title'),
    'body': ('body', 'body1', 'body-md', 'body-lg', 'body-medium', 'text', 'paragraph', 'subtitle1'),
    'mono': ('mono', 'code', 'terminal', 'mono-xs', 'mono-sm', 'mono-md'),
}
RADIUS_ALIASES = {'panel': ('panel', 'hero', 'dialog', 'card', 'xl', 'lg'),
                  'tile': ('tile', 'card', 'md', 'lg'), 'chip': ('chip', 'pill', 'sm')}
COLOR = re.compile(r'#(?:[0-9a-fA-F]{8}|[0-9a-fA-F]{6}|[0-9a-fA-F]{3,4})\b|rgba?\([0-9.,% ]+\)')
SKIP_DIRS = {'node_modules', 'dist', 'build', 'out', 'coverage', 'target', 'vendor'}


# --- A small YAML subset: the DESIGN.md frontmatter shapes design-spec writes.

def _scalar(text):
    text = text.strip()
    if text[:1] in ('"', "'"):
        quote = text[0]
        end = text.find(quote, 1)
        while quote == "'" and end != -1 and text[end + 1:end + 2] == "'":
            end = text.find(quote, end + 2)
        return text[1:end].replace("''", "'") if end != -1 else text[1:]
    return re.sub(r'\s+#.*$', '', text).strip()


def _flow(text, i=0):
    """Parse a flow collection or scalar at text[i]; return (value, next index)."""
    while i < len(text) and text[i] == ' ':
        i += 1
    if i < len(text) and text[i] in '[{':
        closing = ']' if text[i] == '[' else '}'
        items, i = ([] if closing == ']' else {}), i + 1
        while i < len(text):
            while i < len(text) and text[i] in ' ,':
                i += 1
            if i < len(text) and text[i] == closing:
                return items, i + 1
            if closing == ']':
                value, i = _flow(text, i)
                items.append(value)
            else:
                key, i = _flow_token(text, i, ':')
                value, i = _flow(text, i + 1)
                items[_scalar(key)] = value
        return items, i
    token, i = _flow_token(text, i, ',]}')
    return _scalar(token), i


def _flow_token(text, i, stops):
    start, quote = i, None
    while i < len(text):
        c = text[i]
        if quote:
            quote = None if c == quote else quote
        elif c in '"\'' and (i == start or text[i - 1] in ' :'):
            quote = c
        elif c in stops and (c != ':' or text[i + 1:i + 2] in (' ', '')):
            break
        i += 1
    return text[start:i], i


def _block(lines, i, indent):
    """Parse the block at `indent` from lines[i]; return (value, next index)."""
    if i < len(lines) and lines[i][1].startswith('- '):
        items = []
        while i < len(lines) and lines[i][0] == indent and lines[i][1].startswith('- '):
            items.append(_scalar(lines[i][1][2:]))
            i += 1
        return items, i
    mapping = {}
    while i < len(lines) and lines[i][0] == indent:
        match = re.match(r'''("[^"]*"|'[^']*'|[^:]+?):(?:\s+(.*))?$''', lines[i][1])
        if not match:
            i += 1
            continue
        key, rest = _scalar(match.group(1)), (match.group(2) or '').strip()
        i += 1
        if rest and not rest.startswith('#'):
            mapping[key] = _flow(rest)[0] if rest[0] in '[{' else _scalar(rest)
        elif i < len(lines) and (lines[i][0] > indent or (lines[i][0] == indent and lines[i][1].startswith('- '))):
            mapping[key], i = _block(lines, i, lines[i][0])
        else:
            mapping[key] = None
    return mapping, i


def frontmatter(path):
    """Return (parsed frontmatter, raw head text) or (None, '') without one."""
    text = path.read_text(encoding='utf-8').replace('\r\n', '\n')
    closing = re.search(r'\n---[ \t]*(?:\n|$)', text[3:]) if text.startswith('---\n') else None
    if not closing:
        return None, ''
    head = text[4:3 + closing.start() + 1]
    lines = [(len(line) - len(line.lstrip(' ')), line.strip()) for line in head.splitlines()
             if line.strip() and not line.lstrip().startswith('#')]
    return _block(lines, 0, 0)[0], head


# --- Lifted from cas-release-report's render.py (token_json, design_md).

def merge_tokens(base, override):
    for key, value in override.items():
        if isinstance(value, dict) and isinstance(base.get(key), dict):
            merge_tokens(base[key], value)
        else:
            base[key] = value


def family_css(key, family):
    return '--font-' + key + ':' + ','.join('"' + f + '"' if ' ' in f else f for f in family)


def token_json(defaults, project_root, token_path):
    """CSS from the bundled defaults, overlaid by --tokens or design-tokens.json."""
    tokens = json.loads(defaults.read_text())
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
    fonts, families = [], {}
    for key in ('display', 'body', 'mono'):
        family = tokens['typography']['family'][key]['$value']
        if not isinstance(family, list) or not family or any(not re.fullmatch(r'[A-Za-z0-9 _-]+', f) for f in family):
            raise ValueError('font family tokens must be nonempty arrays of local font names')
        fonts.append(family_css(key, family))
        families[key] = {'stack': family, 'face': None}
    light, dark, fonts = colors('light'), colors('dark'), ';'.join(fonts)
    css = (':root{' + light + ';' + fonts + ';color-scheme:light dark}\n'
           '@media(prefers-color-scheme:dark){:root{' + dark + '}}\n'
           '@media print{:root{' + light + ';color-scheme:light}}')
    return css, selected, families


DARK_SCHEME = re.compile(r'@media\s*\(\s*prefers-color-scheme\s*:\s*dark\s*\)')
FONT_FILE = re.compile(r'url\("(\./[\w./-]+\.woff2)"\)')


def design_md(project_root, path, meta, head):
    """Tokens a DESIGN.md names: its CSS `source` files and `maps` roles.

    Returns None unless the file lists at least one existing .css source and
    a maps block of `role: "--variable"` entries. The CSS is inlined as-is
    (relative .woff2 faces become data URIs), so it may hold only custom
    properties, @media/selector blocks and @font-face rules: no network URL
    and no markup. Each role becomes `--role: var(--variable)`.
    """
    sources = meta.get('source')
    sources = [sources] if isinstance(sources, str) else sources if isinstance(sources, list) else []
    maps = re.search(r'^maps:\n((?:[ \t]+.*\n?)+)', head, re.M)
    if not maps:
        return None
    sheets = [project_root / str(item).split()[0] for item in sources if str(item).split() and str(item).split()[0].endswith('.css')]
    sheets = [sheet for sheet in sheets if sheet.is_file()]
    if not sheets:
        return None
    roles = re.findall(r'^\s+([a-z][a-z0-9-]*):\s*"?(--[a-z][\w-]*)"?\s*$', maps.group(1), re.M)
    if not roles:
        return None
    css = []
    for sheet in sheets:
        content = sheet.read_text(encoding='utf-8')
        def font(match):
            face = (sheet.parent / match.group(1)).resolve()
            if not face.is_file() or face.stat().st_size > FACE_BYTES:
                raise ValueError(f'{rel(project_root, path)} font source missing or too large: ' + match.group(1))
            return 'url("data:font/woff2;base64,' + base64.b64encode(face.read_bytes()).decode() + '")'
        content = FONT_FILE.sub(font, content)
        if '</' in content or '@import' in content or re.search(r'url\(\s*(?!["\']?data:)', content):
            raise ValueError(f'{rel(project_root, path)} CSS sources may hold only tokens and local font faces: ' + sheet.name)
        # Print is always the light set: a dark-scheme block applies on screen only.
        css.append(DARK_SCHEME.sub('@media screen and (prefers-color-scheme: dark)', content))
    # A role named like its variable is already defined; aliasing it would be a cycle.
    aliases = ';'.join('--' + role + ':var(' + name + ')' for role, name in roles if '--' + role != name)
    return '\n'.join(css) + '\n:root{' + aliases + '}'


# --- design-spec value keys: colors, typography, rounded.

def color_value(value):
    match = COLOR.search(str(value)) if value is not None else None
    return match.group(0) if match else None


def families(value):
    if isinstance(value, dict):
        value = value.get('fontFamily') or value.get('family') or value.get('$value')
    if isinstance(value, list):
        names = [str(v) for v in value]
    elif isinstance(value, str):
        names = [part.strip().strip('"\'') for part in value.split(',')]
    else:
        return []
    return [name for name in names if name]


def pick_type(typography):
    """Return {role: [family names]} for display, body and mono."""
    if not isinstance(typography, dict):
        return {}
    named = typography.get('families') or typography.get('family')
    if isinstance(named, dict):
        return {role: families(named.get(role)) for role in TYPE_ROLES if families(named.get(role))}
    picked = {}
    for role, keys in TYPE_ROLES.items():
        found = next((families(typography[k]) for k in keys if families(typography.get(k))), None)
        if not found and role == 'mono':
            found = next((families(v) for k, v in typography.items() if ('mono' in k or 'code' in k) and families(v)), None)
        if found:
            picked[role] = found
    if 'body' not in picked:
        first = next((families(v) for v in typography.values() if families(v)), None)
        if first:
            picked['body'] = first
    if 'display' not in picked and 'body' in picked:
        picked['display'] = picked['body']
    return picked


def woff2_files(design_dir):
    """Local .woff2 files under the DESIGN.md directory, outside dependencies and builds."""
    found = []
    for directory, dirs, files in os.walk(design_dir):
        dirs[:] = sorted(d for d in dirs if not d.startswith('.') and d not in SKIP_DIRS)
        found.extend(Path(directory) / name for name in sorted(files) if name.lower().endswith('.woff2'))
    return found


def local_face(faces, family):
    """The .woff2 named for a family, preferring a regular or variable cut."""
    wanted = re.sub(r'[\s_-]', '', family.lower())
    named = [face for face in faces if re.sub(r'[\s_-]', '', face.name.lower()).startswith(wanted)]
    named.sort(key=lambda face: 0 if re.search(r'regular|variable', face.name.lower()) else 1)
    return named[0] if named else None


def value_keys(project_root, chain, warnings):
    """CSS from the merged colors/typography/rounded values, or None without colors."""
    colors, typography, rounded = {}, {}, {}
    for path, meta in reversed(chain):  # ancestor first; the nearer file overrides
        for target, key in ((colors, 'colors'), (typography, 'typography'), (rounded, 'rounded')):
            if isinstance(meta.get(key), dict):
                target.update(meta[key])
    roles = {}
    for role, value in colors.items():
        color = color_value(value)
        if re.fullmatch(r'[a-z][a-z0-9-]*', str(role)) and color:
            roles[role] = color
    if not roles:
        return None
    defaulted = []
    for role, aliases in ROLE_ALIASES.items():
        if role not in roles:
            alias = next((roles[a] for a in aliases if a in roles), None)
            if alias:
                roles[role] = alias
            else:
                defaulted.append(role)
    css_vars = ['--' + role + ':' + value for role, value in roles.items()]
    faces, picked, type_receipt = [], pick_type(typography), {}
    local = woff2_files(chain[0][0].parent)
    for role in ('display', 'body', 'mono'):
        names = [n for n in picked.get(role, []) if re.fullmatch(r'[A-Za-z0-9 _-]+', n)]
        stack = names + ([] if names and names[-1] in GENERIC_FAMILIES else
                         [f for f in FALLBACK_STACK[role] if f not in names])
        face = None
        if names and names[0] not in GENERIC_FAMILIES:
            found = local_face(local, names[0])
            if found and found.stat().st_size <= FACE_BYTES:
                face = rel(project_root, found)
                faces.append('@font-face{font-family:"' + names[0] + '";font-display:swap;src:url("data:font/woff2;base64,'
                             + base64.b64encode(found.read_bytes()).decode() + '") format("woff2")}')
            elif not any(w.startswith(f'no local .woff2 for {names[0]};') for w in warnings):
                warnings.append(f'no local .woff2 for {names[0]}; {role} text falls back to: ' + ', '.join(stack))
        css_vars.append(family_css(role, stack))
        type_receipt[role] = {'stack': stack, 'face': face}
    for name, value in rounded.items():
        if re.fullmatch(r'[a-z0-9][a-z0-9-]*', str(name)) and re.fullmatch(r'[0-9.]+(?:px|rem|em|%)?', str(value)):
            css_vars.append('--radius-' + str(name) + ':' + str(value))
    for role, keys in RADIUS_ALIASES.items():
        value = next((rounded[k] for k in keys if k in rounded and k != role), None)
        if role not in rounded and value is not None and re.fullmatch(r'[0-9.]+(?:px|rem|em|%)?', str(value)):
            css_vars.append('--radius-' + role + ':' + str(value))
    # Value keys carry one scheme: print and screen both wear it.
    css = '\n'.join(faces + [':root{' + ';'.join(css_vars) + ';color-scheme:light}'])
    return css, {'roles': len(roles), 'defaulted': defaulted}, type_receipt


# --- Which DESIGN.md, its inherits chain, revision and mark.

def rel(root, path):
    try:
        return Path(path).resolve().relative_to(root).as_posix()
    except ValueError:
        return str(path)


def revision(root, path):
    try:
        done = subprocess.run(['git', '-C', str(root), 'log', '-1', '--format=%h', '--', rel(root, path)],
                              capture_output=True, text=True, timeout=10)
    except (OSError, subprocess.SubprocessError):
        return None
    return done.stdout.strip() or None


def nested_designs(root):
    found = []
    for pattern in ('*/DESIGN.md', '*/*/DESIGN.md'):
        for path in root.glob(pattern):
            parts = path.relative_to(root).parts[:-1]
            if not any(p.startswith('.') or p in SKIP_DIRS for p in parts):
                found.append(path)
    return sorted(found)


def find_design(root, surface, problems):
    """Return (path, how) for the DESIGN.md the deliverable wears, or (None, None)."""
    profile = root / 'docs/deliverables/PROFILE.md'
    if profile.is_file():
        named = (frontmatter(profile)[0] or {}).get('design')
        if isinstance(named, str) and named.strip():
            path = root / named.strip()
            if path.is_file():
                return path, 'profile'
            problems.append(('profile-design-missing', f'docs/deliverables/PROFILE.md names {named.strip()}, which does not exist'))
    if surface:
        directory = (root / surface).resolve()
        directory = directory.parent if directory.is_file() else directory
        while directory == root or root in directory.parents:
            if (directory / 'DESIGN.md').is_file():
                return directory / 'DESIGN.md', 'surface'
            directory = directory.parent
    if (root / 'DESIGN.md').is_file():
        return root / 'DESIGN.md', 'root'
    nested = nested_designs(root)
    if len(nested) == 1:
        return nested[0], 'nested'
    if nested:
        problems.append(('ambiguous-design-md', 'several DESIGN.md files and none at the root: '
                         + ', '.join(rel(root, p) for p in nested) + '; pass --surface'))
    return None, None


def inherits_chain(root, path, warnings):
    """[(path, meta, head)] from the chosen DESIGN.md through `inherits:` files."""
    chain, seen = [], set()
    while path and path.resolve() not in seen and len(chain) < 6:
        seen.add(path.resolve())
        meta, head = frontmatter(path)
        if meta is None:
            warnings.append(f'{rel(root, path)} has no YAML frontmatter')
            meta = {}
        chain.append((path, meta, head))
        parent = meta.get('inherits')
        path = None
        if isinstance(parent, str) and parent.strip().endswith('.md'):
            path = next((p for p in (root / parent.strip(), chain[-1][0].parent / parent.strip()) if p.is_file()), None)
            if path is None:
                warnings.append(f'{rel(root, chain[-1][0])} inherits {parent.strip()}, which does not exist')
    return chain


def brand_mark(root, chain):
    for path, meta, _ in chain:
        named = meta.get('brand-mark')
        if not isinstance(named, str) or not named.strip():
            continue
        for candidate in (root / named.strip(), path.parent / named.strip()):
            kind = MARK_TYPES.get(candidate.suffix.lower())
            if kind and candidate.is_file() and candidate.stat().st_size <= FACE_BYTES:
                uri = 'data:' + kind + ';base64,' + base64.b64encode(candidate.read_bytes()).decode()
                return {'path': rel(root, candidate), 'bytes': candidate.stat().st_size}, uri
    return None, None


def resolve(project_root, *, defaults, tokens=None, audience='internal', surface=None):
    """Return {receipt, css, mark_uri}; css is None when a finding refuses the brand."""
    root = Path(project_root).resolve()
    warnings, findings, problems = [], [], []
    receipt = {'schema': 'cas.deliverable.brand/1', 'audience': audience, 'root': str(root),
               'source': None, 'design': None, 'tokens': None, 'type': None, 'mark': None}
    css, mark_uri = None, None
    if tokens:
        css, _, receipt['type'] = token_json(defaults, root, Path(tokens))
        receipt['source'], receipt['tokens'] = 'tokens', {'path': str(tokens)}
    else:
        design, how = find_design(root, surface, problems)
        if design:
            chain = inherits_chain(root, design, warnings)
            receipt['design'] = {'path': rel(root, design), 'revision': revision(root, design), 'found_by': how,
                                 'inherits': [{'path': rel(root, p), 'revision': revision(root, p)} for p, _, _ in chain[1:]]}
            final = chain[-1][1].get('inherits')
            if isinstance(final, str) and final.strip() and not final.strip().endswith('.md'):
                receipt['design']['inherits_base'] = final.strip()
            for path, meta, head in chain:
                try:
                    css = design_md(root, path, meta, head)
                except ValueError as error:
                    warnings.append(str(error) + '; using its design-spec value keys')
                if css:
                    receipt['source'], receipt['type'] = 'design-md-css', {'faces_inlined': css.count('data:font/woff2')}
                    receipt['tokens'] = {'from': rel(root, path)}
                    break
            if not css:
                found = value_keys(root, [(p, m) for p, m, _ in chain], warnings)
                if found:
                    css, receipt['tokens'], receipt['type'] = found
                    receipt['source'] = 'design-md-values'
                else:
                    problems.append(('design-md-unusable', f'{rel(root, design)} has neither CSS sources with maps '
                                     'nor colors values the deliverable can wear'))
            if css:
                receipt['mark'], mark_uri = brand_mark(root, chain)
                if not receipt['mark']:
                    warnings.append(f'{rel(root, design)} names no brand-mark the deliverable can inline')
        elif not problems:
            problems.append(('no-design-md', f'no DESIGN.md in {root}; run design-spec to create one'))
    if css is None:
        if audience == 'customer':
            findings.extend({'code': code, 'message': message + '; a customer deliverable never wears the bundled defaults'}
                            for code, message in problems)
        else:
            warnings.extend(message for _, message in problems)
            css, selected, receipt['type'] = token_json(defaults, root, None)
            receipt['source'] = 'design-tokens-json' if selected else 'defaults'
            receipt['tokens'] = {'path': rel(root, selected)} if selected else None
    receipt['warnings'], receipt['findings'] = warnings, findings
    return {'receipt': receipt, 'css': css, 'mark_uri': mark_uri}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', default=Path('.'), type=Path)
    parser.add_argument('--defaults', required=True, type=Path)
    parser.add_argument('--tokens', type=Path)
    parser.add_argument('--surface', type=Path)
    parser.add_argument('--audience', choices=('internal', 'customer'), default='internal')
    args = parser.parse_args()
    try:
        result = resolve(args.root, defaults=args.defaults, tokens=args.tokens,
                         audience=args.audience, surface=args.surface)
    except (ValueError, KeyError, OSError, TypeError) as error:
        parser.exit(2, 'brand: ' + str(error) + '\n')
    json.dump(result, sys.stdout)


if __name__ == '__main__':
    main()
