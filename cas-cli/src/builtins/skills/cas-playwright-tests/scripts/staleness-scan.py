#!/usr/bin/env python3
"""Static staleness scan: grade each Playwright spec against the current app sources.

Read-only and dependency-free. For every spec (and the relative helpers it
imports) it extracts navigated routes, API paths, test ids, locator classes and
data-* attributes, resolves each against the current sources, and grades the
spec CURRENT / UNSURE / UNCHECKED / STALE / DEAD with a file:line receipt per dependency.
No browser, no target, no model spend. Grades are a starting point: read each
non-CURRENT spec and settle the verdict by citing the current source line.

Extension points: add a function to ROUTE_ADAPTERS or API_ADAPTERS that takes
(root, path) and returns [(pattern, file, line)]; patterns use `:p`, `{p}`,
`[p]` for one segment and `*`, `**`, `{*p}`, `[...p]` for the rest.
"""
import argparse
from collections import defaultdict
import fnmatch
import json
from pathlib import Path
import re
import sys

EXCLUDE = {'.git', '.cas', 'target', 'dist', 'node_modules', 'vendor', 'build', '__pycache__', '.venv', '.nuxt', '.output'}
SOURCE_SUFFIXES = {'.ts', '.tsx', '.js', '.jsx', '.mjs', '.cjs', '.mts', '.vue', '.svelte', '.html', '.css', '.scss', '.rs', '.py'}
IMPORT_SUFFIXES = ('', '.ts', '.tsx', '.mts', '.js', '.mjs', '.jsx', '/index.ts', '/index.js')
LITERAL = re.compile(r"""(['"`])((?:\\.|(?!\1).)*?)\1""")
NAVIGATION = re.compile(r"""\b(?:goto|toHaveURL|waitForURL)\(\s*(['"`])((?:\\.|(?!\1).)*?)\1""")
TEST_ID_CALL = re.compile(r"""getByTestId\(\s*(['"`])((?:\\.|(?!\1).)*?)\1""")
TEST_ID_ATTR = re.compile(r"""data-testid\s*=\s*\\?['"]?([\w${}.:-]+)""")
LOCATOR = re.compile(r"""\.locator\(\s*(['"`])((?:\\.|(?!\1).)*?)\1""")
IMPORT = re.compile(r"""(?:\bfrom\s*|\bimport\s*\(\s*|\brequire\s*\(\s*|^\s*import\s+)(['"])(\.{1,2}/[^'"]+)\1""", re.M)
INTERPOLATION = re.compile(r'\$\{[^}]*\}')
TEST_FILE = re.compile(r'(^|/)(__tests__|__mocks__|e2e|tests?)/|\.(test|spec)\.[cm]?[jt]sx?$')
HOLE = '\x00'


def walk(base, suffixes=SOURCE_SUFFIXES):
    if base.is_file():
        yield base
        return
    for path in sorted(base.rglob('*')):
        if path.is_file() and path.suffix in suffixes and not EXCLUDE.intersection(path.relative_to(base).parts):
            yield path


def lines_of(path):
    try:
        return path.read_text(errors='replace').splitlines()
    except OSError:
        return []


def compile_pattern(route):
    """Turn a framework route into segments: ('rest',), ('optional', regex) or ('one', regex)."""
    segments = []
    for part in [p for p in route.strip().split('/') if p]:
        if re.fullmatch(r'\*\*|\*\w*|\{\*\w*\}|\[\.\.\.\w+\]', part):
            segments.append(('rest', None))
        elif re.fullmatch(r'\[\[\w+\]\]', part):
            segments.append(('optional', re.compile('[^/]+')))
        else:
            seg = re.sub(r':\w+|\\\{\w+\\\}|\\\[\w+\\\]', '[^/]+', re.escape(part))
            segments.append(('one', re.compile(seg)))
    return segments


def segment_match(parts, segments, prefix):
    """Match path segments exactly; with prefix=True the last part may be the start of a segment."""
    if not parts:
        return prefix or all(kind != 'one' for kind, _ in segments)
    if not segments:
        return False
    kind, regex = segments[0]
    head, last = parts[0], len(parts) == 1
    if kind == 'rest':
        return True
    if head == HOLE or regex.fullmatch(head) or (prefix and last and (regex.pattern == '[^/]+' or regex.pattern.startswith(re.escape(head)))):
        if segment_match(parts[1:], segments[1:], prefix):
            return True
    return kind == 'optional' and segment_match(parts, segments[1:], prefix)


def route_nuxt(root, path):
    """Nuxt pages tree: [param], [[optional]], [...rest], (group). routeRules redirects in
    nuxt.config resolve their source path."""
    base = root / path
    out = []
    for file in walk(base, {'.vue', '.tsx', '.jsx', '.ts', '.js'}):
        rel = file.relative_to(base).with_suffix('').as_posix()
        parts = [p for p in rel.split('/') if not re.fullmatch(r'\(.+\)', p)]
        if parts and parts[-1] == 'index':
            parts.pop()
        out.append(('/' + '/'.join(parts), file, 1))
    for config in sorted(base.parent.glob('nuxt.config.*')):
        for n, line in enumerate(lines_of(config), 1):
            match = re.search(r"""['"](/[^'"]*)['"]\s*:\s*\{[^}]*\bredirect\b""", line)
            if match:
                out.append((match.group(1), config, n))
    return out


def route_static(root, path):
    """Static or Vite root: each index.html serves its directory, other .html files their path."""
    base = root / path
    out = []
    for file in walk(base, {'.html'}):
        rel = file.relative_to(base).as_posix()
        out.append(('/' + (rel[:-len('index.html')] if rel.endswith('index.html') else rel), file, 1))
    return out


def api_axum(root, path):
    """Rust axum routers: .route("/v1/x/{id}", ...). Nested .nest() prefixes are not joined."""
    out = []
    for file in walk(root / path, {'.rs'}):
        text = '\n'.join(lines_of(file))
        for match in re.finditer(r'\.route\(\s*"(/[^"]*)"', text):
            out.append((match.group(1), file, text.count('\n', 0, match.start(1)) + 1))
    return out


def api_nestjs(root, path):
    """NestJS: @Controller('prefix') joined with @Get/@Post/... paths; setGlobalPrefix applies to all."""
    base = root / path
    files = list(walk(base, {'.ts', '.js'}))
    prefix = ''
    for file in files:
        match = re.search(r"""setGlobalPrefix\(\s*['"]([^'"]*)['"]""", '\n'.join(lines_of(file)))
        if match:
            prefix = match.group(1).strip('/')
    out = []
    for file in files:
        controller = None
        for n, line in enumerate(lines_of(file), 1):
            match = re.search(r"""@Controller\(\s*(?:\{[^}]*path\s*:\s*)?(?:['"]([^'"]*)['"])?""", line)
            if match:
                controller = (match.group(1) or '').strip('/')
                continue
            match = re.search(r"""@(?:Get|Post|Put|Patch|Delete|All|Options|Head)\(\s*(?:['"]([^'"]*)['"])?""", line)
            if match and controller is not None:
                parts = [prefix, controller, (match.group(1) or '').strip('/')]
                out.append(('/' + '/'.join(p for p in parts if p), file, n))
    return out


def any_list(root, path):
    """One route pattern per line; '#' starts a comment."""
    file = root / path
    return [(line.split('#')[0].strip(), file, n) for n, line in enumerate(lines_of(file), 1) if line.split('#')[0].strip()]


ROUTE_ADAPTERS = {'nuxt': route_nuxt, 'static': route_static, 'axum': api_axum, 'list': any_list}
API_ADAPTERS = {'nestjs': api_nestjs, 'axum': api_axum, 'list': any_list}


def load_patterns(root, specs, adapters):
    """None when the index was omitted (its dependencies stay unchecked); a list, possibly empty, otherwise."""
    if not specs:
        return None
    table = []
    for spec in specs:
        name, _, path = spec.partition(':')
        if name not in adapters or not path:
            raise SystemExit(f'unknown adapter {spec!r}; choose {sorted(adapters)} as name:path')
        for route, file, line in adapters[name](root, path):
            if re.match(r'/*(\*|\{\*|\[\.\.\.)', route):
                continue  # a root catch-all (SPA fallback, CORS preflight) would resolve every path
            table.append((compile_pattern(route), route, f'{file.relative_to(root).as_posix()}:{line}'))
    return table


def normalize(value):
    """Strip origin, query, hash and a leading ${BASE}; interpolations become one-segment holes."""
    value = re.sub(r'^\$\{[^}]*\}', '', value)
    value = re.sub(r'^[a-z]+://[^/]+', '', value)
    value = re.sub(r'\*+', HOLE, INTERPOLATION.sub(HOLE, value)).split('?')[0].split('#')[0]
    if value.startswith('./'):
        value = value[1:]
    return value or '/'


def resolve_path(value, table):
    """An exact path must match one route. Only a declared prefix (a glob or template tail, or a
    startsWith argument, all ending in a hole) may match the start of a route."""
    prefix = value.endswith(HOLE)
    stem = value[:-1] if prefix else value
    parts = [p for p in stem.split('/') if p] + ([''] if prefix and stem.endswith('/') else [])
    parts = [p for p in parts if p or prefix]
    for segments, route, where in table:
        if segment_match(parts, segments, prefix):
            return 'ok', where
    return 'missing', None


class Sources:
    def __init__(self, root, dirs, skip):
        self.files = [(p, '\n'.join(lines_of(p))) for d in dirs for p in walk(root / d)
                      if p.resolve() not in skip and not TEST_FILE.search(p.relative_to(root).as_posix())]
        self.root = root

    def find(self, regex, needle):
        for path, text in self.files:
            match = needle in text and regex.search(text)
            if match:
                return f'{path.relative_to(self.root).as_posix()}:{text.count(chr(10), 0, match.start()) + 1}'
        return None


def resolve_token(kind, value, sources):
    if kind == 'testid':
        static = value.split(HOLE)[0]
        if HOLE not in value:
            where = sources.find(re.compile(r"""(?<![\w-])['"`]?""" + re.escape(value) + r"""['"`](?![\w-])|data-testid=""" + re.escape(value) + r'(?![\w-])'), value)
            if where:
                return 'ok', where
        if HOLE in value and static:
            where = sources.find(re.compile(r"""['"`]""" + re.escape(static)), static)
            if where:
                return 'ok', where
        # An id built at runtime (`syntax-${kind}`, field.replaceAll('_', '-')) leaves a prefix or name to find.
        for cut in [i + 1 for i, ch in enumerate(static) if ch in '-_']:
            where = sources.find(re.compile(r'([\'"`])' + re.escape(static[:cut]) + r'(\$\{|\1\s*\+)'), static[:cut])
            if where:
                return 'unsure', where
        camel = re.sub(r'-(\w)', lambda m: m.group(1).upper(), static)
        for variant in [v for v in (static.replace('-', '_'), camel) if v != static]:
            where = sources.find(re.compile(r'(?<![\w-])' + re.escape(variant) + r'(?![\w-])'), variant)
            if where:
                return 'unsure', where
        return 'missing', None
    if kind == 'class':
        where = sources.find(re.compile(r'(?<![\w-])' + re.escape(value) + r'(?![\w-])'), value)
        return ('ok', where) if where else ('missing', None)
    camel = re.sub(r'-(\w)', lambda m: m.group(1).upper(), value)
    where = sources.find(re.compile(r'data-' + re.escape(value) + r'(?![\w-])|dataset\.' + re.escape(camel) + r'\b'), '')
    return ('ok', where) if where else ('unsure', None)  # library code may set it at runtime


def follow(spec, root, app_dirs):
    """The spec plus the relative helpers it imports; app sources it imports are resolved, not scanned."""
    seen, queue = [], [spec]
    while queue:
        path = queue.pop()
        if path in seen:
            continue
        seen.append(path)
        for match in IMPORT.finditer('\n'.join(lines_of(path))):
            target = (path.parent / match.group(2))
            stem = target.with_suffix('') if target.suffix in {'.js', '.mjs'} else target
            for candidate in [Path(str(target) + s) for s in IMPORT_SUFFIXES] + [Path(str(stem) + '.ts')]:
                resolved = candidate.resolve()
                if candidate.is_file() and root in resolved.parents and 'node_modules' not in candidate.parts \
                        and not any(d in resolved.parents for d in app_dirs):
                    queue.append(resolved)
                    break
    return seen


def extract(path, root, api_prefix):
    rel = path.relative_to(root).as_posix()
    deps = []
    for n, line in enumerate(lines_of(path), 1):
        if re.match(r'\s*(//|/\*|\*)', line):
            continue
        where = f'{rel}:{n}'
        navigated = set()
        for match in NAVIGATION.finditer(line):
            value = match.group(2)
            navigated.add(value)
            if not re.match(r'(about|data|blob|javascript):', value):
                route = normalize(value)
                deps.append(('api' if api_prefix.match(route) else 'route', route, where))
        for match in LITERAL.finditer(line):
            value = normalize(match.group(2)) if match.group(2) else ''
            if match.group(2) not in navigated and api_prefix.match(value):
                if re.search(r'\.startsWith\(\s*$', line[:match.start()]) and not value.endswith(HOLE):
                    value += HOLE  # a startsWith argument declares a prefix
                deps.append(('api', value, where))
        for match in list(TEST_ID_CALL.finditer(line)) + list(TEST_ID_ATTR.finditer(line)):
            value = INTERPOLATION.sub(HOLE, match.group(match.lastindex))
            if value:
                deps.append(('testid', value, where))
        for match in LOCATOR.finditer(line):
            selector = re.sub(r"""(['"]).*?\1""", '', match.group(2))
            for cls in re.findall(r'(?<![\w\-\]])\.([A-Za-z_][\w-]*)', selector):
                deps.append(('class', cls, where))
            for attr in re.findall(r'\[data-([\w-]+)', selector):
                if attr != 'testid':
                    deps.append(('data', attr, where))
    return deps


def kind_of(path, text):
    if '.journey.' in path.name or 'test.step(' in text:
        return 'journey'
    if re.search(r'regression|\bGH #\d+|#\d{3,}', text, re.I):
        return 'regression'
    return 'one-off'


ACTION = {'UNCHECKED': 'rerun with the missing index', 'CURRENT': 'keep', 'UNSURE': 'keep after grepping the setter', 'STALE': 'rewrite', 'DEAD': 'delete or rewrite'}


def scan(args):
    root = Path(args.root).resolve()
    api_prefix = re.compile(args.api_prefix)
    routes = load_patterns(root, args.routes, ROUTE_ADAPTERS)
    apis = load_patterns(root, args.api, API_ADAPTERS)
    specs = sorted({p.resolve() for g in args.specs for p in root.glob(g) if p.is_file() and not EXCLUDE.intersection(p.relative_to(root).parts)})
    if args.exclude:
        specs = [p for p in specs if not any(fnmatch.fnmatch(p.relative_to(root).as_posix(), g) for g in args.exclude)]
    app_dirs = [(root / d).resolve() for d in args.sources]
    closures = {spec: follow(spec, root, app_dirs) for spec in specs}
    skip = {p for files in closures.values() for p in files}
    sources = Sources(root, args.sources, skip) if args.sources else None
    cache, rows, findings = {}, [], defaultdict(set)
    for spec in specs:
        counts, deps = defaultdict(int), []
        for file in closures[spec]:
            for kind, value, where in extract(file, root, api_prefix):
                key = (kind, value)
                if key not in cache:
                    table = routes if kind == 'route' else apis
                    if kind in ('route', 'api'):
                        cache[key] = ('unchecked', None) if table is None else resolve_path(value, table)
                    else:
                        cache[key] = ('unchecked', None) if sources is None else resolve_token(kind, value, sources)
                verdict, receipt = cache[key]
                counts[verdict] += 1
                deps.append({'kind': kind, 'value': value.replace(HOLE, '${}'), 'verdict': verdict, 'spec_line': where, 'source_line': receipt})
                if verdict in ('missing', 'unsure'):
                    findings[(kind, value.replace(HOLE, '${}'), verdict, where, receipt or '')].add(spec.relative_to(root).as_posix())
        if counts['missing'] and not counts['ok']:
            grade = 'DEAD'
        elif counts['missing']:
            grade = 'STALE'
        elif counts['unchecked']:
            grade = 'UNCHECKED'
        elif counts['unsure']:
            grade = 'UNSURE'
        else:
            grade = 'CURRENT'
        rows.append({'spec': spec.relative_to(root).as_posix(), 'grade': grade, 'kind': kind_of(spec, '\n'.join(lines_of(spec))),
                     'action': ACTION[grade], 'ok': counts['ok'], 'missing': counts['missing'], 'unsure': counts['unsure'],
                     'unchecked': counts['unchecked'], 'dependencies': deps})
    found = [{'kind': k, 'value': v, 'verdict': verdict, 'where': where, 'candidate': candidate or None, 'specs': sorted(s)}
             for (k, v, verdict, where, candidate), s in sorted(findings.items(), key=lambda item: (item[0][2], item[0][3]))]
    return {'root': str(root), 'specs': rows, 'findings': found,
            'totals': {g: sum(r['grade'] == g for r in rows) for g in ('CURRENT', 'UNSURE', 'UNCHECKED', 'STALE', 'DEAD')}}


def markdown(report):
    out = ['| Spec | Grade | Kind (suggested) | Action (suggested) | Resolved | Missing | Unsure | Unchecked |',
           '| --- | --- | --- | --- | ---: | ---: | ---: | ---: |']
    for row in report['specs']:
        out.append(f"| {row['spec']} | {row['grade']} | {row['kind']} | {row['action']} | {row['ok']} | {row['missing']} | {row['unsure']} | {row['unchecked']} |")
    out += ['', '| Spec | Dependency | Kind | Verdict | Referenced at | Source receipt |', '| --- | --- | --- | --- | --- | --- |']
    for row in report['specs']:
        for d in row['dependencies']:
            value = d['value'].replace('|', '\\|')
            out.append(f"| {row['spec']} | `{value}` | {d['kind']} | {d['verdict']} | {d['spec_line']} | {d['source_line'] or '-'} |")
    totals = report['totals']
    out += ['', f"{len(report['specs'])} specs: " + ', '.join(f'{totals[g]} {g}' for g in totals)]
    return '\n'.join(out)


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('--root', default='.', help='project root; every other path is relative to it')
    parser.add_argument('--specs', action='append', required=True, help='spec glob, repeatable (e.g. e2e/**/*.spec.ts)')
    parser.add_argument('--exclude', action='append', default=[], help='spec glob to skip, repeatable')
    parser.add_argument('--routes', action='append', default=[], help=f'page routes as adapter:path; adapters {sorted(ROUTE_ADAPTERS)}')
    parser.add_argument('--api', action='append', default=[], help=f'API routes as adapter:path; adapters {sorted(API_ADAPTERS)}')
    parser.add_argument('--api-prefix', default=r'^/(?:api|v\d+)(?:/|$)', help='regex selecting API paths among string literals')
    parser.add_argument('--sources', action='append', default=[], help='frontend source dir for test ids, classes and data-* (repeatable)')
    parser.add_argument('--json', action='store_true', help='emit the full JSON report')
    args = parser.parse_args()
    report = scan(args)
    print(json.dumps(report, indent=2) if args.json else markdown(report))
    return 0


if __name__ == '__main__':
    sys.exit(main())
