#!/usr/bin/env python3
"""Rule-026: token-based, conservative process-state test lint and strict ratchet.

No Rust is compiled or executed. See docs/qa/test-env-lint.md for the analysis
boundary and baseline review policy.
"""
import argparse
from collections import defaultdict
from dataclasses import dataclass
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tomllib

from rust_test_source import pairs, tokens

BASELINE = 'scripts/test-env-baseline.json'
CANONICAL = 'cas-cli/src/test_env_guard.rs'
MUTATORS = {'set_var', 'remove_var', 'set_current_dir'}
CONSTRUCTORS = {'new', 'temp_home', 'with_vars', 'with_optional_vars'}


def git(root, *args):
    result = subprocess.run(['git', *args], cwd=root, text=True, capture_output=True)
    if result.returncode:
        raise ValueError(result.stderr.strip() or 'Git query failed')
    return result.stdout


@dataclass
class Function:
    unit: object
    name: str
    scope: tuple
    start: int
    body: int
    end: int
    attrs: tuple
    test: bool
    params: tuple
    returns_guard: bool

    @property
    def label(self):
        return '::'.join((*self.scope, self.name))


class Unit:
    def __init__(self, path, source):
        self.path, self.source = path, source
        self.ts = tokens(source)
        self.ps = pairs(self.ts)
        self.functions = []
        self.call_cache, self.binding_cache = {}, {}
        self.aliases = {'TestEnvGuard': 'TestEnvGuard', 'env': 'std::env'}
        self.local_guard = any(self.values(i, i + 2) == ['struct', 'TestEnvGuard']
                               for i in range(len(self.ts))) and path != CANONICAL
        self._imports()
        self._items(0, len(self.ts), (), any(part == 'tests' or part.endswith('_tests') for part in Path(path).parts) or
                    Path(path).stem == 'tests' or Path(path).stem.endswith(('_test', '_tests')))

    def values(self, a, b):
        return [t.value for t in self.ts[a:b] if not t.string]

    def _imports(self):
        # Flatten brace imports, retaining aliases and module renames. Names
        # are resolved before matching calls, never inside strings/comments.
        def expand(a, b, prefix=()):
            k, part = a, []
            while k < b:
                v = self.ts[k].value
                if v == '{':
                    expand(k + 1, self.ps[k], (*prefix, *part))
                    part = []
                    k = self.ps[k] + 1
                    continue
                if v == ',':
                    save((*prefix, *part))
                    part = []
                elif v != '::':
                    part.append(v)
                k += 1
            save((*prefix, *part))

        def save(parts):
            if not parts or '*' in parts:
                return
            if 'as' in parts:
                pos = parts.index('as')
                name, path = parts[pos + 1], parts[:pos]
            else:
                name, path = parts[-1], parts
            self.aliases[name] = '::'.join(path)

        for i, t in enumerate(self.ts):
            if t.value == 'use':
                j = i + 1
                while j < len(self.ts) and self.ts[j].value != ';':
                    j += 1
                expand(i + 1, j)

    def _items(self, a, b, scope, test_context):
        i, attrs = a, []
        while i < b:
            v = self.ts[i].value
            if v == '#' and i + 1 < b and self.ts[i + 1].value == '[':
                end = self.ps[i + 1]
                attrs.append(''.join(self.values(i + 2, end)))
                i = end + 1
                continue
            if v == 'fn' and i + 2 < b:
                name = self.ts[i + 1].value
                p = i + 2
                while p < b and self.ts[p].value not in ('(', ';', '{'):
                    p += 1
                if p >= b or self.ts[p].value != '(':
                    i += 1
                    continue
                pend = self.ps[p]
                j = pend + 1
                while j < b and self.ts[j].value not in ('{', ';'):
                    j += 1
                if j < b and self.ts[j].value == '{':
                    end = self.ps[j]
                    f = Function(self, name, scope, i, j, end, tuple(attrs),
                                 test_context or any(x in ('test', 'tokio::test') or
                                                     x.startswith('tokio::test(') or
                                                     (x.startswith('cfg(') and re.search(r'\btest\b', x)) for x in attrs),
                                 tuple(self.values(p + 1, pend)),
                                 any(self.aliases.get(v, v).split('::')[-1] == 'TestEnvGuard'
                                     for v in self.values(pend + 1, j)) and
                                 not any(v in ('<', 'impl') for v in self.values(pend + 1, j)))
                    self.functions.append(f)
                    # Named inner helpers must not execute just by declaration.
                    self._items(j + 1, end, (*scope, name), f.test)
                    attrs = []
                    i = end + 1
                    continue
            if v in ('mod', 'impl'):
                j = i + 1
                while j < b and self.ts[j].value not in ('{', ';'):
                    j += 1
                if j < b and self.ts[j].value == '{':
                    header = self.values(i + 1, j)
                    label = header[0] if v == 'mod' else (header[header.index('for') + 1]
                                                         if 'for' in header else header[0])
                    self._items(j + 1, self.ps[j], (*scope, label), test_context or
                                any(x.startswith('cfg(') and re.search(r'\btest\b', x) for x in attrs))
                    i, attrs = self.ps[j] + 1, []
                    continue
            if v == '{':
                # Macro-generated test containers still contain literal items.
                self._items(i + 1, self.ps[i], scope, test_context)
                i = self.ps[i] + 1
                attrs = []
                continue
            if v == ';':
                attrs = []
            i += 1

    def local_binding(self, f, name, site):
        if id(f) not in self.binding_cache:
            bindings = []
            for k, value in enumerate(f.params):
                if value == ':' and k:
                    bindings.append((f.params[k - 1], f.body, f.end))
            scopes = [f.end]
            for k in range(f.body + 1, f.end):
                value = self.ts[k].value
                if not self.ts[k].string and value == '{':
                    scopes.append(self.ps[k])
                elif not self.ts[k].string and value == '}':
                    scopes.pop()
                elif not self.ts[k].string and value == 'let':
                    # Conditional patterns need control-flow name resolution;
                    # do not let them suppress a later free helper call.
                    if self.ts[k - 1].value in ('if', 'while', '&&'):
                        continue
                    pos = k + 1
                    if self.ts[pos].value == 'mut':
                        pos += 1
                    candidate = self.ts[pos].value
                    # Simple lexical declarations/parameters shadow free
                    # helpers after the initializer. Closure bodies themselves
                    # are still visited; pattern bindings remain conservative.
                    if (not re.fullmatch(r'[A-Za-z_]\w*', candidate) or
                            self.ts[pos + 1].value not in ('=', ':')):
                        continue
                    end = pos + 1
                    while end < scopes[-1] and self.ts[end].value != ';':
                        if not self.ts[end].string and self.ts[end].value in ('(', '[', '{'):
                            end = self.ps[end]
                        end += 1
                    if end < scopes[-1]:
                        bindings.append((candidate, end, scopes[-1]))
            self.binding_cache[id(f)] = bindings
        return any(bound == name and begin < site < end
                   for bound, begin, end in self.binding_cache[id(f)])

    def call(self, i):
        if i not in self.call_cache:
            self.call_cache[i] = self._call(i)
        return self.call_cache[i]

    def _call(self, i):
        if self.ts[i].string or not re.fullmatch(r'[A-Za-z_]\w*', self.ts[i].value):
            return None
        if i > 1 and self.ts[i - 1].value == '::' and re.fullmatch(r'[A-Za-z_]\w*', self.ts[i - 2].value):
            return None
        names, j = [self.ts[i].value], i + 1
        while j + 1 < len(self.ts) and self.ts[j].value == '::':
            if self.ts[j + 1].value == '<':
                depth, j = 1, j + 2
                while j < len(self.ts) and depth:
                    depth += (self.ts[j].value == '<') - (self.ts[j].value == '>')
                    j += 1
                if depth:
                    return None
            else:
                names.append(self.ts[j + 1].value)
                j += 2
        if j >= len(self.ts) or self.ts[j].value != '(':
            return None
        alias = self.aliases.get(names[0], names[0]).split('::')
        return '::'.join((*alias, *names[1:])), j, self.ps[j]


class Analyzer:
    def __init__(self, sources):
        self.units = [Unit(path, src) for path, src in sorted(sources.items())]
        self.functions = [f for u in self.units for f in u.functions]
        self.by_name = defaultdict(list)
        for f in self.functions:
            self.by_name[f.name].append(f)
        self.findings = {}
        self.memo, self.stack = {}, set()
        self.active, self.unsafe, self.nesting = [], {}, set()
        self.resolve_cache, self.raw_cache = {}, {}
        self.by_scope = defaultdict(list)
        for f in self.functions:
            self.by_scope[(id(f.unit), f.scope)].append(f)

    def record(self, f, i, kind, detail):
        u = f.unit
        call = u.call(i)
        end = call[2] + 1 if call else i + 1
        signature = ' '.join(('literal:' if t.string else '') + t.value for t in u.ts[i:end])
        # Include literal arguments, exclude whitespace/comments, and count
        # repeated identical sites within a named function. Line numbers are
        # display only: unrelated line edits do not churn the ratchet.
        digest = hashlib.sha256(signature.encode()).hexdigest()[:16]
        equivalent = []
        for j in range(f.body + 1, i + 1):
            other = u.call(j)
            if other and ' '.join(('literal:' if t.string else '') + t.value for t in u.ts[j:other[2] + 1]) == signature:
                equivalent.append(j)
        occurrence = len(equivalent) or 1
        ident = f'{u.path}::{f.label}::{kind}::{digest}::{occurrence}'
        self.findings[ident] = {'id': ident, 'path': u.path,
                              'line': u.source.count('\n', 0, u.ts[i].start) + 1,
                              'function': f.label, 'kind': kind, 'detail': detail,
                              'signature': signature,
                              'canonical_impl': u.path == CANONICAL and bool(f.scope) and f.scope[-1] in ('TestEnvGuard', 'AmbientEnvRestore'),
                              'ignored_test': any(x.startswith('ignore') for x in f.attrs)
                              and any(x == 'test' for x in f.attrs)}
        if kind in ('unguarded-mutation', 'unguarded-helper-call', 'nested-guard', 'nested-helper-call') and self.active:
            self.unsafe[self.active[-1]] = True
            if kind.startswith('nested'):
                self.nesting.add(self.active[-1])
        return ident

    def resolve(self, f, path, site=None):
        if site is not None and '::' not in path and f.unit.local_binding(f, path, site):
            return []
        key = (id(f), path)
        if key not in self.resolve_cache:
            self.resolve_cache[key] = self._resolve(f, path)
        return self.resolve_cache[key]

    def _resolve(self, f, path):
        parts = path.split('::')
        name = parts[-1]
        candidates = self.by_name.get(name, [])
        if len(parts) > 1:
            owner = parts[-2]
            if owner == 'Self' and f.scope:
                owner = f.scope[-1]
            candidates = [x for x in candidates if owner in x.scope or
                          Path(x.unit.path).stem == owner]
        else:
            # Receiver method calls are not guessed from a common method name.
            candidates = [x for x in candidates if x.unit is f.unit or not x.scope or not x.scope[-1][:1].isupper()]
        same_file = [x for x in candidates if x.unit is f.unit]
        if same_file:
            candidates = same_file
        crate = f.unit.path.split('/src/')[0].split('/tests/')[0]
        candidates = [x for x in candidates if x.unit.path.startswith(crate + '/')]
        # Prefer the closest lexical owner; keep ambiguous candidates rather
        # than silently assuming the least restrictive helper.
        if len(parts) == 1 and candidates and same_file:
            score = lambda x: sum(a == b for a, b in zip(x.scope, f.scope))
            best = max(map(score, candidates))
            candidates = [x for x in candidates if score(x) == best]
        return candidates

    def callback(self, f, start, end, guards, site):
        words = f.unit.values(start, end)
        if words and all(re.fullmatch(r'[A-Za-z_]\w*|::', x) for x in words):
            for callee in self.resolve(f, ''.join(words), site):
                self.called(f, callee, guards, site)

    def called(self, caller, callee, guards, site):
        protected = bool(guards) and '@maybe-dropped' not in guards
        self.analyze(callee, protected)
        if self.unsafe.get((id(callee), protected)):
            kind = 'nested-helper-call' if (id(callee), protected) in self.nesting else 'unguarded-helper-call'
            self.record(caller, site, kind, callee.label)

    def raw_support(self, f):
        if id(f) not in self.raw_cache:
            self.raw_cache[id(f)] = f.name == 'drop' or any(
                call and call[0] in {f'std::env::{v}' for v in MUTATORS}
                for call in (f.unit.call(k) for k in range(f.body + 1, f.end)))
        return self.raw_cache[id(f)]

    def analyze(self, f, incoming=False):
        key = (id(f), incoming)
        if key in self.memo:
            return self.memo[key]
        if key in self.stack:
            return f.returns_guard
        self.stack.add(key)
        self.active.append(key)
        u, guards = f.unit, set()
        if incoming:
            guards.add('@caller')
        # Only direct guard parameters own a lock; Option<Guard> does not.
        for pos, param in enumerate(f.params):
            resolved = u.aliases.get(param, param).split('::')[-1]
            if resolved == 'TestEnvGuard' and not u.local_guard:
                left = f.params[max(0, pos - 4):pos]
                if ':' in left and '<' not in left:
                    colon = max(k for k in range(pos) if f.params[k] == ':')
                    guards.add(f.params[colon - 1])
        # Drop impls restore under their existing owner; only the canonical
        # implementation gets exact reviewed exceptions, never a file bypass.
        if f.unit.path == CANONICAL and f.scope and f.scope[-1] == 'TestEnvGuard':
            guards.add('@canonical')
        self.walk(f, f.body + 1, f.end, guards)
        self.stack.remove(key)
        self.active.pop()
        self.memo[key] = f.returns_guard
        return f.returns_guard

    def binding(self, u, a, i):
        j = i - 1
        while j >= a and u.ts[j].value not in (';', '{', '}'):
            j -= 1
        before = u.values(j + 1, i)
        if 'let' in before and '=' in before:
            pos = before.index('let') + 1
            if before[pos] == 'mut':
                pos += 1
            return before[pos]
        return None

    def walk(self, f, a, b, guards):
        u, i = f.unit, a
        while i < b:
            if u.ts[i].value == 'fn':
                nested = next((x for x in u.functions if x.start == i), None)
                if nested:
                    i = nested.end + 1
                    continue
            call = u.call(i)
            if call:
                path, p, end = call
                names = path.split('::')
                method = i > 0 and u.ts[i - 1].value == '.'
                shared = 'TestEnvGuard' in names and not u.local_guard
                if not method and path in {f'std::env::{x}' for x in MUTATORS}:
                    if u.path == CANONICAL and f.scope and f.scope[-1] in ('TestEnvGuard', 'AmbientEnvRestore'):
                        self.record(f, i, 'implementation-mutation', path)
                    elif not guards or '@maybe-dropped' in guards:
                        self.record(f, i, 'unguarded-mutation', path)
                elif shared and names[-1] in CONSTRUCTORS:
                    if guards:
                        self.record(f, i, 'nested-guard', path)
                    self.walk(f, p + 1, end, guards.copy())
                    binding = self.binding(u, a, i)
                    if binding:
                        guards.add(binding)
                    # A discarded constructor lives through this statement.
                    else:
                        guards.add('@temporary')
                    i = end + 1
                    continue
                elif shared and names[-1] == 'run_with_temp_home':
                    if guards:
                        self.record(f, i, 'nested-guard', path)
                    self.walk(f, p + 1, end, guards | {'@callback'})
                    self.callback(f, p + 1, end, guards | {'@callback'}, i)
                    i = end + 1
                    continue
                elif path in ('std::thread::spawn', 'thread::spawn', 'tokio::spawn'):
                    # Child thread/task does not inherit the parent's ownership.
                    self.walk(f, p + 1, end, set())
                    self.callback(f, p + 1, end, set(), i)
                    i = end + 1
                    continue
                elif path in ('drop', 'std::mem::drop'):
                    arg = u.values(p + 1, end)
                    if len(arg) == 1:
                        guards.discard(arg[0])
                        if not guards - {'@maybe-dropped'}:
                            guards.clear()
                elif method and i > 1 and u.ts[i - 2].value not in guards:
                    for callee in self.by_name.get(names[-1], []):
                        if callee.unit is u and callee.scope and callee.scope[-1][:1].isupper():
                            self.called(f, callee, guards, i)
                elif not method:
                    for callee in self.resolve(f, path, i):
                        self.called(f, callee, guards, i)
                        if len(path.split('::')) > 1 and callee.scope and callee.scope[-1][:1].isupper():
                            for owned in self.by_scope[(id(callee.unit), callee.scope)]:
                                if owned.scope == callee.scope and owned is not callee and self.raw_support(owned):
                                    self.called(f, owned, guards, i)
                        if callee.returns_guard:
                            binding = self.binding(u, a, i)
                            guards.add(binding or '@temporary')
                # Visit args/closures conservatively on the same thread.
                self.walk(f, p + 1, end, guards.copy())
                i = end + 1
                continue
            if u.ts[i].value == '{':
                child = guards.copy()
                self.walk(f, i + 1, u.ps[i], child)
                # Drops in an unconditional lexical block affect existing
                # owners; conditional drops cannot prove ownership absent.
                begin = i - 1
                while begin >= a and u.ts[begin].value not in (';', '{', '}'):
                    begin -= 1
                prefix = u.values(begin + 1, i)
                conditional = any(v in prefix for v in ('if', 'else', 'match', 'for', 'while', 'loop'))
                if not conditional:
                    guards.intersection_update(child)
                    if '@maybe-dropped' in child:
                        guards.add('@maybe-dropped')
                elif guards - child:
                    # May still own the lock (nesting is unsafe), but cannot
                    # establish protection for a later process mutation.
                    guards.add('@maybe-dropped')
                i = u.ps[i] + 1
                continue
            if u.ts[i].value == ';':
                guards.discard('@temporary')
            i += 1

    def run(self):
        for f in self.functions:
            if f.test or f.unit.path == CANONICAL:
                self.analyze(f)
        return sorted(self.findings.values(), key=lambda x: x['id'])


def workspace_sources(root):
    manifest = root / 'Cargo.toml'
    # Real workspace membership, including future members; fixtures may use a
    # miniature workspace. Vendored, non-member sources are outside this lint.
    data = tomllib.loads(manifest.read_text()) if manifest.exists() else {}
    members = data.get('workspace', {}).get('members', ['cas-cli', 'crates/*'])
    roots = [p for pattern in members for p in root.glob(pattern) if p.is_dir()]
    files = git(root, 'ls-files', '--cached', '--others', '--exclude-standard', '-z').split('\0')
    return {name: (root / name).read_text() for name in sorted(set(files))
            if name.endswith('.rs') and (root / name).is_file() and
            any((root / name).is_relative_to(p) for p in roots)}


def read_baseline(data):
    if not isinstance(data, dict) or set(data) != {'version', 'violations', 'exceptions'} or type(data['version']) is not int or data['version'] != 1:
        raise ValueError('invalid baseline schema')
    result = {}
    for section in ('violations', 'exceptions'):
        if not isinstance(data[section], list):
            raise ValueError('baseline entries must be lists')
        for row in data[section]:
            if not isinstance(row, dict) or set(row) != {'id', 'reason'} or not isinstance(row['id'], str) or not isinstance(row['reason'], str) or not row['reason'].strip():
                raise ValueError('every exact baseline entry needs an id and nonempty review reason')
            if row['id'] in result:
                raise ValueError('duplicate baseline id: ' + row['id'])
            result[row['id']] = section
    return result


def ratchet(findings, baseline):
    allowed = read_baseline(baseline)
    actual = {row['id']: row for row in findings}
    errors = []
    for ident in sorted(actual.keys() - allowed.keys()):
        row = actual[ident]
        errors.append(f"{row['path']}:{row['line']}: {row['kind']} in {row['function']} ({row['detail']}) [{ident}]")
    for ident in sorted(allowed.keys() - actual.keys()):
        errors.append('stale baseline entry: ' + ident)
    for ident in sorted(allowed.keys() & actual.keys()):
        row = actual[ident]
        if allowed[ident] == 'exceptions' and not (row['canonical_impl'] or row['ignored_test']):
            errors.append('exception requires canonical implementation or ignored subprocess test: ' + ident)
    return errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=Path.cwd())
    parser.add_argument('--changed-since', help='ratchet against this Git baseline; still scans the whole workspace')
    parser.add_argument('--inventory', action='store_true', help='print findings; never update the baseline')
    args = parser.parse_args()
    try:
        root = args.root.resolve()
        findings = Analyzer(workspace_sources(root)).run()
        if args.inventory:
            print(json.dumps(findings, indent=2))
            return 0
        baseline = json.loads((root / BASELINE).read_text())
        errors = ratchet(findings, baseline)
        current = read_baseline(baseline)
        # HEAD comparison covers unstaged growth; changed-since covers committed
        # lane growth. Initial installation has no previous baseline to compare.
        seed_history = git(root, 'log', '--reverse', '--format=%H', '--', BASELINE).splitlines()
        refs = ['HEAD'] + ([args.changed_since] if args.changed_since else [])
        if seed_history:
            refs.extend(seed_history)
        for ref in dict.fromkeys(refs):
            git(root, 'rev-parse', '--verify', ref + '^{commit}')
            probe = subprocess.run(['git', 'cat-file', '-e', f'{ref}:{BASELINE}'], cwd=root, capture_output=True)
            if probe.returncode == 0:
                old = read_baseline(json.loads(git(root, 'show', f'{ref}:{BASELINE}')))
                for ident in sorted(current.keys() - old.keys()):
                    errors.append(f'baseline growth since {ref}: {ident}')
                for ident in current.keys() & old.keys():
                    if current[ident] != old[ident]:
                        errors.append(f'baseline disposition changed since {ref}: {ident}')
        for error in errors:
            print('test-env: ' + error, file=sys.stderr)
        print(f'test-env: {len(findings)} findings, {len(baseline["violations"])} reasoned legacy violations, '
              f'{len(baseline["exceptions"])} exact exceptions, {len(errors)} errors')
        return int(bool(errors))
    except (ValueError, OSError, KeyError, IndexError) as error:
        print('test-env: ' + str(error), file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
