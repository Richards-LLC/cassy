#!/usr/bin/env python3
"""Five concurrent, zero-retry executions of touched Rust tests, in the worker node.

Planning reads immutable Git blobs only. No compiler/test runs occur at park.
The embedded runner supplies the same canonical lexer used by repository tools.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import glob
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import tempfile
import time

if 'tokens' not in globals():
    from rust_test_source import tokens, pairs


def git(root, *args, missing=False):
    result = subprocess.run(['git', '-C', str(root), *args], capture_output=True,
                            text=True, timeout=20)
    if result.returncode and not missing:
        raise ValueError(result.stderr.strip() or 'Git query failed')
    return '' if result.returncode else result.stdout


def items(source):
    """Test functions and external module declarations; strings/comments are inert."""
    ts = tokens(source)
    ps = pairs(ts)
    tests, modules = [], []

    def walk(left, right, scope=(), inline=()):
        i, attrs, attr_start = left, [], None
        while i < right:
            t = ts[i]
            if t.string:
                i += 1
                continue
            if t.value == '#' and i + 1 < right and ts[i+1].value == '[':
                end = ps[i+1]
                attrs.append(ts[i+2:end])
                if attr_start is None:
                    attr_start = i
                i = end + 1
                continue
            if t.value == 'mod' and i + 2 < right:
                name, after = ts[i+1].value, ts[i+2].value
                if after == ';':
                    path = next((a[2].value.strip('"') for a in attrs
                                 if len(a) == 3 and a[0].value == 'path' and a[1].value == '='), None)
                    modules.append((scope, inline, name, path))
                    i += 3
                elif after == '{':
                    end = ps[i+2]
                    walk(i+3, end, (*scope, name), (*inline, name))
                    i = end + 1
                else:
                    i += 1
                attrs, attr_start = [], None
                continue
            if t.value == 'fn' and i + 2 < right:
                name = ts[i+1].value
                opening = next((j for j in range(i+2, right) if not ts[j].string and ts[j].value in ('(', ';', '{')), right)
                if opening < right and ts[opening].value == '(':
                    body = ps[opening] + 1
                    while body < right and (ts[body].string or ts[body].value not in ('{', ';')):
                        body += 1
                    if body < right and ts[body].value == '{':
                        end = ps[body]
                        labels = [''.join(t.value for t in a if not t.string).split('(')[0] for a in attrs]
                        if any(a in ('test', 'tokio::test', 'async_std::test', 'rstest', 'test_case') for a in labels):
                            begin = i if attr_start is None else attr_start
                            signature = [(t.value, t.string) for t in ts[begin:end+1]]
                            tests.append({'function': '::'.join((*scope, name)),
                                          'fingerprint': hashlib.sha256(json.dumps(signature).encode()).hexdigest()})
                        i = end + 1
                        attrs, attr_start = [], None
                        continue
            if t.value in ('{', '(', '[') and i in ps:
                i = ps[i] + 1
                attrs, attr_start = [], None
            elif t.value == ';':
                i += 1
                attrs, attr_start = [], None
            else:
                i += 1
    walk(0, len(ts))
    return tests, modules


def plan(root, base, head):
    head = git(root, 'rev-parse', head).strip()
    base = git(root, 'merge-base', base, head).strip()
    changed = git(root, 'diff', '--name-only', '--no-renames', '--diff-filter=AM', '-z', base, head, '--').split('\0')
    tests = []
    for path in sorted(p for p in changed if p.endswith('.rs')):
        before = {f['function']: f['fingerprint'] for f in items(git(root, 'show', f'{base}:{path}', missing=True))[0]}
        for test in items(git(root, 'show', f'{head}:{path}'))[0]:
            if before.get(test['function']) != test['fingerprint']:
                tests.append({**test, 'file': path, 'id': path+'::'+test['function']})
    return {'version': 1, 'base': base, 'head': head, 'tests': tests}


def package_root(root, name):
    import tomllib
    pending = [root / 'Cargo.toml']
    visited = set()
    while pending:
        manifest = pending.pop()
        if manifest in visited:
            continue
        visited.add(manifest)
        data = tomllib.loads(manifest.read_text())
        if data.get('package', {}).get('name') == name:
            return manifest.parent, data
        for member in data.get('workspace', {}).get('members', []):
            pending.extend(Path(p) for p in glob.glob(str(manifest.parent / member / 'Cargo.toml')))
    raise ValueError('unknown selected package: '+name)


def source_graph(root, package, harness, target=None):
    if target is not None:
        if target['package'] != package or target['harness'] != harness:
            raise ValueError('worker target metadata does not match requested scope')
        entry = Path(target['entry'])
    else:
        directory, manifest = package_root(root, package)
        if harness is None:
            entry = directory / manifest.get('lib', {}).get('path', 'src/lib.rs')
        else:
            configured = next((x['path'] for x in manifest.get('test', []) if x['name'] == harness and 'path' in x), None)
            entry = directory / (configured or f'tests/{harness}.rs')
    pending, found, cached = [(entry.resolve(), ())], {}, {}
    while pending:
        path, prefix = pending.pop()
        if prefix in found.get(path, []):
            continue
        if not path.is_relative_to(root) or not path.is_file():
            continue
        found.setdefault(path, []).append(prefix)
        if path not in cached:
            source = path.read_text()
            cached[path] = items(source)[1] if re.search(r'\bmod\s+\w+\s*;', source) else []
        mod_dir = path.parent if path.stem in ('lib', 'main', 'mod') else path.with_suffix('')
        for scope, inline, name, explicit in cached[path]:
            context = mod_dir.joinpath(*inline)
            if explicit:
                child = (context if inline else path.parent) / explicit
            else:
                child = context / (name+'.rs')
                if not child.is_file():
                    child = context / name / 'mod.rs'
            pending.append((child.resolve(), (*prefix, *scope, name)))
    return found


def matches(name, expression):
    def term(part):
        match = re.fullmatch(r'\s*test\((=?)([A-Za-z0-9_:.-]+)\)\s*', part)
        if not match:
            raise ValueError('unsupported named worker filter: '+expression)
        return name == match[2] if match[1] else match[2] in name
    return any(all(term(part) for part in group.split('&')) for group in expression.split('|'))


def discovery(command):
    """Bound and clean up this owned listing process, including descendants."""
    child, pending = None, None
    watched = [signal.SIGTERM, signal.SIGINT]
    if hasattr(signal, 'SIGHUP'):
        watched.append(signal.SIGHUP)
    def stop():
        if child is not None and child.returncode is None:
            try:
                if os.name == 'posix':
                    os.killpg(child.pid, signal.SIGKILL)
                else:
                    child.kill()
            except ProcessLookupError:
                pass
    def interrupted(signum, _frame):
        nonlocal pending
        pending = signum
        if child is not None:
            stop()
            child.communicate(timeout=3)
            raise SystemExit(128+signum)
    previous = {s: signal.signal(s, interrupted) for s in watched}
    try:
        child = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                 text=True, close_fds=False, start_new_session=os.name == 'posix')
        if pending is not None:
            interrupted(pending, None)
        try:
            out, err = child.communicate(timeout=float(os.environ.get('CAS_WORKER_NEXTEST_DISCOVERY_TIMEOUT_SECS', '120')))
        except subprocess.TimeoutExpired:
            stop()
            child.communicate(timeout=3)
            raise ValueError('touched-test discovery timed out; owned listing processes stopped')
        if child.returncode:
            raise ValueError('touched-test discovery failed: '+err)
        return json.loads(out)
    finally:
        for sig, handler in previous.items():
            signal.signal(sig, handler)


def selected(root, touched, requested, cargo, target=None):
    if not touched['tests']:
        return [], []
    package = requested[requested.index('-p')+1]
    harness = requested[requested.index('--test')+1] if '--test' in requested else None
    graph = source_graph(root, package, harness, target)
    candidates = [t for t in touched['tests'] if (root / t['file']).resolve() in graph]
    if not candidates:
        return [], []
    index = requested.index('-E')
    listing = discovery([cargo, 'nextest', 'list', *requested[2:index], '--message-format', 'json'])
    names = {}
    for suite in listing['rust-suites'].values():
        for name, info in suite['testcases'].items():
            if name in names:
                raise ValueError('ambiguous compiled test name: '+name)
            names[name] = info
    identities, exact = [], set()
    for test in candidates:
        expected = {'::'.join((*prefix, test['function'])) for prefix in graph[(root / test['file']).resolve()]}
        resolved = [name for name in names if any(name == target or name.startswith(target+'::') for target in expected)]
        if not resolved:
            raise ValueError('cannot resolve touched Rust test '+test['id']+' in selected target; platform/macro proof required')
        selected_names = [name for name in resolved if matches(name, requested[index+1])]
        if not selected_names:
            continue
        for name in selected_names:
            if names[name].get('ignored'):
                raise ValueError('touched test is ignored; explicit platform/provider proof required: '+test['id'])
            if not re.fullmatch(r'[A-Za-z0-9_:.-]+', name):
                raise ValueError('cannot encode exact touched-test name: '+name)
        identities.append(test['id'])
        exact.update(selected_names)
    return sorted(identities), sorted(exact)


def verified(script, cargo, args, count, output=None):
    env = {**os.environ, 'CARGO': str(cargo), 'VERIFIED_TEST_COUNT_FILE': str(count)}
    return subprocess.run(['bash', '-c', script, 'worker-stability-tests', *args],
                          env=env, stdout=output, stderr=subprocess.STDOUT if output else None,
                          close_fds=False).returncode


def run_requested(root, base, requested, cargo, script, count, target=None):
    count = Path(count)
    receipt = count.with_suffix('.stability.json')
    receipt.unlink(missing_ok=True)
    touched = plan(root, base, git(root, 'rev-parse', 'HEAD').strip())
    # Compile and execute the user's requested affected scope once, as before.
    result = verified(script, cargo, requested, count)
    if result:
        raise SystemExit(result)
    identities, names = selected(root, touched, requested, cargo, target)
    if not identities:
        return
    directory = Path(tempfile.mkdtemp(prefix='stability-', dir=count.parent))
    exact_args = [*requested[:requested.index('-E')], '-E', ' | '.join('test(='+name+')' for name in names),
                  '--retries', '0', '--test-threads', '1', '--no-fail-fast']
    import threading
    ready = threading.Barrier(5)
    def trial(number):
        output = directory / f'run-{number}.log'
        trial_count = directory / f'count-{number}'
        with output.open('w') as stream:
            ready.wait(timeout=30)
            started = time.monotonic()
            code = verified(script, cargo, exact_args, trial_count, stream)
            ended = time.monotonic()
        return {'exit_code': code, 'count': int(trial_count.read_text()) if code == 0 and trial_count.exists() else 0,
                'started': started, 'ended': ended, 'log': str(output)}
    load_before = os.getloadavg() if hasattr(os, 'getloadavg') else None
    with ThreadPoolExecutor(max_workers=5) as pool:
        trials = list(pool.map(trial, range(1,6)))
    for number, result in enumerate(trials, 1):
        print(f'--- touched-test stability {number}/5: {result["log"]} ---', flush=True)
        print(Path(result['log']).read_text(), flush=True)
    if any(t['exit_code'] != 0 or t['count'] != len(names) for t in trials):
        raise ValueError('touched-test stability failed; original failed copies retained, no retries; park has no stability receipt')
    if max(t['started'] for t in trials) >= min(t['ended'] for t in trials):
        raise ValueError('five touched-test runs did not overlap; no concurrent stability receipt recorded')
    proof = {**touched, 'tests': identities, 'names': names, 'runs': 5, 'parallelism': 5,
             'retries': 0, 'trials': trials, 'load_before': load_before,
             'load_after': os.getloadavg() if hasattr(os, 'getloadavg') else None}
    receipt.write_text(json.dumps(proof)+'\n')
    print(f'stability: PASS {touched["head"]} tests={len(identities)} runs=5 parallelism=5 retries=0 receipt={receipt}', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--plan', action='store_true')
    parser.add_argument('--base', required=True)
    parser.add_argument('--head')
    parser.add_argument('--cargo')
    parser.add_argument('--verified-script')
    parser.add_argument('--count-file')
    parser.add_argument('--target-metadata', default=os.environ.get('CAS_WORKER_STABILITY_TARGET'))
    parser.add_argument('args', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    root = Path.cwd().resolve()
    if args.plan:
        print(json.dumps(plan(root, args.base, args.head or 'HEAD')))
    else:
        requested = args.args[1:] if args.args[:1] == ['--'] else args.args
        run_requested(root, args.base, requested, args.cargo, args.verified_script, args.count_file,
                      json.loads(args.target_metadata) if args.target_metadata else None)


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, KeyError, subprocess.SubprocessError) as error:
        print('touched-test stability: '+str(error), file=sys.stderr)
        sys.exit(1)
