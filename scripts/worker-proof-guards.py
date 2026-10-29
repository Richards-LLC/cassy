#!/usr/bin/env python3
"""Cheap guards before requested tests; independent counts prevent false proofs."""
import argparse
import ast
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time
import types
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
            yield start, body


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
        removed.update({value for _, value in literals(shape, old, path)} -
                       {value for _, value in literals(shape, new, path)})
    hits = set()
    if not removed:
        return []
    # format! receipts often have tests pinning one rendered field rather
    # than the full template, e.g. "To: cosmic-bear" from "To: {target}".
    fragments = {part.strip() for value in removed if '\n' in value or '{' in value
                 for part in re.split(r'\{[^{}]*\}|\n', value)
                 if len(part.strip()) >= 3}
    needles = removed | fragments
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
        for start, value in literals(shape, source, path):
            if value not in removed and not any(fragment in value for fragment in fragments):
                continue
            preceding = [name for name in names if name.start() < start]
            if rust:
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
            hits.add((path, source.count('\n', 0, start) + 1, label))
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
    parser.add_argument('--stability-script')
    parser.add_argument('--target-metadata', default=os.environ.get('CAS_WORKER_STABILITY_TARGET'))
    parser.add_argument('--head')
    parser.add_argument('--red', action='store_true')
    parser.add_argument('args', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    repo = Path.cwd()
    for sidecar in ('.stability.json', '.red.json'):
        Path(args.count_file).with_suffix(sidecar).unlink(missing_ok=True)
    base = git(repo, 'merge-base', args.base, 'HEAD').strip()
    started = time.monotonic()
    subprocess.run([sys.executable, str(repo / 'scripts/check-test-shape.py'),
                    '--changed-since', base], check=True)
    hits = changed_string_matches(repo, base)
    for path, line, name in hits:
        print(f'Worker changed-string warning: {path}:{line} test {name}; run or update this assertion before push', flush=True)
    with tempfile.TemporaryDirectory(prefix='worker-proof-guards-') as temporary:
        guard_count = Path(temporary) / 'count'
        verified(args.verified_script, args.cargo,
                 ['nextest', 'run', '-p', 'cas', '--lib', '-E', GUARD_FILTER], guard_count)
        print(f'guards: PASS tests={guard_count.read_text().strip()} '
              f'elapsed={time.monotonic() - started:.3f}s string_matches={len(hits)}', flush=True)
    requested = args.args[1:] if args.args[:1] == ['--'] else args.args
    stability = types.ModuleType('worker_test_stability')
    sys.modules[stability.__name__] = stability
    source = args.stability_script
    if source is None:
        source = Path(__file__).with_name('worker-test-stability.py').read_text()
    exec(source, stability.__dict__)
    stability.run_requested(repo.resolve(), base, requested, args.cargo,
                            args.verified_script, args.count_file,
                            json.loads(args.target_metadata) if args.target_metadata else None,
                            args.head, args.red)


if __name__ == '__main__':
    main()
