#!/usr/bin/env python3
"""Cheap guards before requested tests; independent counts prevent false proofs."""
import argparse
import ast
import importlib.util
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time
import warnings

GUARD_FILTER = ('test(=test_support::lib_test_process_env_mutation_is_isolated) | '
                'test(=config::mod_tests::serialized_factory_fields_have_registry_entries_cas_f7b9)')


def git(repo, *args):
    return subprocess.check_output(['git', '-C', str(repo), *args], text=True)


def load_shape(repo):
    sys.path.insert(0, str(repo / 'scripts'))
    spec = importlib.util.spec_from_file_location('worker_test_shape', repo / 'scripts/check-test-shape.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def literals(shape, source, path):
    for start, end, body in shape.text_literals(source, path):
        quoted = source[start:end]
        if not re.match(r'(?:b|c)?r#*"', quoted):
            try:
                with warnings.catch_warnings():
                    warnings.simplefilter('ignore', SyntaxWarning)
                    body = ast.literal_eval(quoted)
            except (ValueError, SyntaxError):
                pass
        if isinstance(body, str) and body.strip():
            yield start, end, body


def removed_spans(repo, base, path, source):
    """Old-side hunk spans; a multiline literal may start before its hunk."""
    offsets = [0]
    for line in source.splitlines(keepends=True):
        offsets.append(offsets[-1] + len(line))
    diff = git(repo, 'diff', '--no-ext-diff', '--no-color', '--unified=0', base, '--', path)
    spans = []
    for match in re.finditer(r'^@@ -(\d+)(?:,(\d+))? ', diff, re.M):
        start, count = int(match[1]), int(match[2] or 1)
        if count:
            spans.append((offsets[start - 1], offsets[start + count - 1]))
    return spans


def schema_literal(value):
    """DDL is one schema contract, never a bag of shared column fragments."""
    return re.match(r'\s*(?:--[^\n]*\n\s*)*(?:CREATE|ALTER|DROP)\s+(?:TABLE|INDEX)\b',
                    value, re.I) is not None


def assertion_spans(shape, source):
    ts = shape.tokens(source)
    ends = shape.pairs(ts)
    return [(ts[i + 2].start, ts[ends[i + 2]].end)
            for i in range(len(ts) - 2)
            if re.fullmatch(r'(?:debug_|prop_)?assert\w*|expect', ts[i].value)
            and ts[i + 1].value == '!' and i + 2 in ends]


def changed_string_matches(repo, base):
    """Name surviving test references to removed literal values, including raw strings."""
    shape = load_shape(repo)
    removed = set()
    paths = git(repo, 'diff', '--name-only', '--diff-filter=MD', '-z', base, '--').split('\0')
    for path in filter(None, paths):
        if Path(path).suffix not in {'.rs', '.py', '.js', '.ts', '.tsx', '.jsx', '.mjs', '.cjs'}:
            continue
        old = git(repo, 'show', f'{base}:{path}')
        current = repo / path
        new = current.read_text() if current.is_file() else ''
        changed = removed_spans(repo, base, path, old)
        removed.update({value for start, end, value in literals(shape, old, path)
                        if any(start < right and end > left for left, right in changed)} -
                       {value for _, _, value in literals(shape, new, path)})
    hits = set()
    if not removed:
        return []
    # format! receipts often have tests pinning one rendered field rather
    # than the full template, e.g. "To: cosmic-bear" from "To: {target}".
    fragments = {part.strip() for value in removed
                 if not schema_literal(value) and ('\n' in value or '{' in value)
                 for part in re.split(r'\{[^{}]*\}|\n', value)
                 if len(part.strip()) >= 3}
    fragments = sorted(fragments, key=lambda value: (-len(value), value))
    needles = removed | set(fragments)
    for path in filter(None, git(repo, 'ls-files', '-z').split('\0')):
        source_path = repo / path
        if not source_path.is_file() or path.startswith('vendor/'):
            continue
        rust = path.endswith('.rs')
        if source_path.suffix not in {'.rs', '.py', '.js', '.ts', '.tsx', '.jsx', '.mjs', '.cjs', '.sh', '.bash'}:
            continue
        if not rust and not shape.is_test_file(path):
            continue
        try:
            source = source_path.read_text()
        except UnicodeDecodeError:
            continue
        # Fast grep before tokenizing candidate Rust files.
        if not any(value in source or repr(value)[1:-1] in source for value in needles):
            continue
        names = list(re.finditer(r'\b(?:async\s+)?fn\s+([A-Za-z_][A-Za-z_0-9]*)\s*\(', source)) if rust else []
        assertions = None
        for start, end, value in literals(shape, source, path):
            matched = value if value in removed else next((part for part in fragments if part in value), None)
            if matched is None:
                continue
            preceding = [name for name in names if name.start() < start]
            if rust:
                if schema_literal(value):
                    if assertions is None:
                        assertions = assertion_spans(shape, source)
                    if not any(left <= start and end <= right for left, right in assertions):
                        continue
                if not preceding:
                    continue
                name = preceding[-1]
                # Inline and integration test functions carry a test attribute.
                prefix_start = preceding[-2].end() if len(preceding) > 1 else 0
                if not re.search(r'#\[(?:(?:tokio|rstest)::)?(?:test|rstest)(?:\]|\()', source[prefix_start:name.start()]):
                    if not shape.is_test_file(path):
                        continue
                label = name[1]
            else:
                prefix_lines = source[:start].splitlines()
                label = (prefix_lines[-1].strip()[:100] if prefix_lines else '') or '<test file>'
            hits.add((path, source.count('\n', 0, start) + 1, label, matched))
    return sorted(hits)


def verified(script, cargo, args, count):
    env = os.environ.copy()
    env.update(CARGO=cargo, VERIFIED_TEST_COUNT_FILE=str(count))
    # The build target lease must survive Python and Bash into Cargo/rustc.
    result = subprocess.run(['bash', '-c', script, 'worker-verified-tests', *args],
                            env=env, close_fds=False)
    if result.returncode:
        raise SystemExit(result.returncode)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--base', required=True)
    parser.add_argument('--verified-script', required=True)
    parser.add_argument('--cargo', required=True)
    parser.add_argument('--count-file', required=True)
    parser.add_argument('args', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    repo = Path.cwd()
    base = git(repo, 'merge-base', args.base, 'HEAD').strip()
    started = time.monotonic()
    subprocess.run([sys.executable, str(repo / 'scripts/check-test-shape.py'),
                    '--changed-since', base], check=True)
    hits = changed_string_matches(repo, base)
    for path, line, name, fragment in hits:
        print(f'Worker changed-string warning: {path}:{line} test {name}; run or update this assertion before push'
              f'; matched {fragment!r}', flush=True)
    with tempfile.TemporaryDirectory(prefix='worker-proof-guards-') as temporary:
        guard_count = Path(temporary) / 'count'
        verified(args.verified_script, args.cargo,
                 ['nextest', 'run', '-p', 'cas', '--lib', '-E', GUARD_FILTER], guard_count)
        print(f'guards: PASS tests={guard_count.read_text().strip()} '
              f'elapsed={time.monotonic() - started:.3f}s string_matches={len(hits)}', flush=True)
    requested = args.args[1:] if args.args[:1] == ['--'] else args.args
    verified(args.verified_script, args.cargo, requested, args.count_file)


if __name__ == '__main__':
    main()
