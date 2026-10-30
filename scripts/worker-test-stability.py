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
            if t.value == 'include' and i + 2 < right and ts[i+1].value == '!' and ts[i+2].value == '(':
                end = ps[i+2]
                if end == i+4 and ts[i+3].string:
                    literal = ts[i+3].value
                    # Ordinary literal includes inherit this namespace. Macro-
                    # computed paths remain unresolved and cannot satisfy park.
                    if literal.startswith('"'):
                        modules.append((scope, inline, None, json.loads(literal)))
                i = end + 1
                attrs, attr_start = [], None
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
                        kinds = [label.split('::')[-1] for label in labels]
                        if any(a in ('test', 'rstest', 'test_case') for a in kinds):
                            begin = i if attr_start is None else attr_start
                            signature = [(t.value, t.string) for t in ts[begin:end+1]]
                            tests.append({'function': '::'.join((*scope, name)),
                                          'parameterized': any(a in ('rstest', 'test_case') for a in kinds),
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


def package_root(root, name, sources=()):
    import tomllib
    pending = [root / 'Cargo.toml'] if (root / 'Cargo.toml').is_file() else []
    for source in sources:
        for parent in (root / source).parents:
            if not parent.is_relative_to(root):
                break
            manifest = parent / 'Cargo.toml'
            if manifest.is_file():
                pending.append(manifest)
                break
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


def source_graph(root, package, harness, target=None, sources=()):
    if target is not None:
        if target['package'] != package or target['harness'] != harness:
            raise ValueError('worker target metadata does not match requested scope')
        entry = Path(target['entry'])
    else:
        directory, manifest = package_root(root, package, sources)
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
            cached[path] = items(source)[1] if re.search(r'\bmod\s+\w+\s*;|\binclude\s*!', source) else []
        mod_dir = path.parent if path.stem in ('lib', 'main', 'mod') else path.with_suffix('')
        for scope, inline, name, explicit in cached[path]:
            if name is None:
                pending.append(((path.parent / explicit).resolve(), (*prefix, *scope)))
                continue
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
    cases = selected_cases(root, touched, requested, cargo, target)
    return sorted(cases), sorted({name for names in cases.values() for name in names})


def selected_cases(root, touched, requested, cargo, target=None):
    """Touched test id -> the exact compiled names the requested filter runs."""
    if not touched['tests']:
        return {}
    package = requested[requested.index('-p')+1]
    harness = requested[requested.index('--test')+1] if '--test' in requested else None
    graph = source_graph(root, package, harness, target, (t['file'] for t in touched['tests']))
    candidates = [t for t in touched['tests'] if (root / t['file']).resolve() in graph]
    if not candidates:
        return {}
    index = requested.index('-E')
    listing = discovery([cargo, 'nextest', 'list', *requested[2:index], '--message-format', 'json'])
    names = {}
    for suite in listing['rust-suites'].values():
        for name, info in suite['testcases'].items():
            if name in names:
                raise ValueError('ambiguous compiled test name: '+name)
            names[name] = info
    cases = {}
    for test in candidates:
        expected = {'::'.join((*prefix, test['function'])) for prefix in graph[(root / test['file']).resolve()]}
        resolved = [name for name in names if any(name == target or
                    (test.get('parameterized', False) and name.startswith(target+'::')) for target in expected)]
        if not resolved:
            raise ValueError('cannot resolve touched Rust test '+test['id']+' in selected target; platform/macro proof required')
        selected_names = [name for name in resolved if matches(name, requested[index+1])]
        if not selected_names:
            continue
        if len(selected_names) != len(resolved):
            raise ValueError('filter selects only some cases of touched Rust test '+test['id']+'; select the whole test function')
        for name in selected_names:
            if names[name].get('ignored'):
                raise ValueError('touched test is ignored; explicit platform/provider proof required: '+test['id'])
            if not re.fullmatch(r'[A-Za-z0-9_:.-]+', name):
                raise ValueError('cannot encode exact touched-test name: '+name)
        cases[test['id']] = sorted(selected_names)
    return cases


# nextest status lines, with or without the "(i/n)" counter, which nextest
# pads to the total's width: "(  2/120)". LEAK is a pass that leaked handles.
STATUS = re.compile(r'^\s*(PASS|LEAK|FAIL|TIMEOUT|SIGSEGV|SIGABRT|SIGKILL|SIGBUS|ABORT|LEAK-FAIL)'
                    r'\s+\[\s*([0-9.]+)s\]\s+(?:\(\s*\d+/\d+\)\s+)?\S+\s+(\S+)\s*$', re.M)
PASSED = ('PASS', 'LEAK')


def outcomes(path):
    text = re.sub(r'\x1b\[[0-9;]*[A-Za-z]', '', Path(path).read_text(errors='replace'))
    return [(m[1], m[3], float(m[2])) for m in STATUS.finditer(text)]


def record_red(root, touched, requested, cargo, target, log, receipt):
    """Bind a failing requested run to the touched tests it ran, for park's fail-first check."""
    try:
        cases = selected_cases(root, touched, requested, cargo, target)
    except (ValueError, KeyError, OSError, subprocess.SubprocessError) as error:
        # A build failure cannot list tests; the test failure itself still stands.
        print('red: not recorded: '+str(error), flush=True)
        return
    failed = {name for status, name, _ in outcomes(log) if status not in PASSED}
    tests = [{'id': t['id'], 'fingerprint': t['fingerprint'], 'names': cases[t['id']],
              'failed': sorted(failed & set(cases[t['id']]))}
             for t in touched['tests'] if t['id'] in cases]
    if not any(t['failed'] for t in tests):
        print('red: not recorded: no touched test failed in this run', flush=True)
        return
    receipt.write_text(json.dumps({'version': 1, 'base': touched['base'], 'head': touched['head'],
                                   'tests': tests,
                                   'log_sha256': hashlib.sha256(Path(log).read_bytes()).hexdigest()})+'\n')
    print(f'red: FAIL {touched["head"]} tests={",".join(t["id"] for t in tests if t["failed"])}', flush=True)


def verified(script, cargo, args, count, output=None, log=None):
    env = {**os.environ, 'CARGO': str(cargo), 'VERIFIED_TEST_COUNT_FILE': str(count)}
    if log is not None:
        env['VERIFIED_TEST_LOG'] = str(log)
    return subprocess.run(['bash', '-c', script, 'worker-stability-tests', *args],
                          env=env, stdout=output, stderr=subprocess.STDOUT if output else None,
                          close_fds=False).returncode


def run_requested(root, base, requested, cargo, script, count, target=None, head=None, red=False):
    """`head` names the tested commit (a red-control snapshot when `red`)."""
    count = Path(count)
    receipt = count.with_suffix('.stability.json')
    red_receipt = count.with_suffix('.red.json')
    log = count.with_suffix('.requested.log')
    for stale in (receipt, red_receipt):
        stale.unlink(missing_ok=True)
    touched = plan(root, base, head or git(root, 'rev-parse', 'HEAD').strip())
    if touched['tests'] and not (root / 'Cargo.toml').is_file() and '--manifest-path' not in requested:
        # Rust metadata avoids requiring tomllib on older Python installations.
        manifest = target.get('manifest') if target is not None else None
        if manifest is None:
            package = requested[requested.index('-p')+1]
            directory, _ = package_root(root, package, (t['file'] for t in touched['tests']))
            manifest = str(directory / 'Cargo.toml')
        index = requested.index('-E')
        requested = [*requested[:index], '--manifest-path', str(manifest), *requested[index:]]
    # Compile and execute the user's requested affected scope once, as before.
    try:
        result = verified(script, cargo, requested, count, log=log)
        if result:
            record_red(root, touched, requested, cargo, target, log, red_receipt)
            raise SystemExit(result)
    finally:
        log.unlink(missing_ok=True)
    if red:
        raise ValueError('red control: the named tests passed under the mutation; no red receipt recorded')
    cases = selected_cases(root, touched, requested, cargo, target)
    identities, names = sorted(cases), sorted({name for found in cases.values() for name in found})
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
    durations = {name: [] for name in names}
    for result in trials:
        for status, name, seconds in outcomes(result['log']):
            if status in PASSED and name in durations:
                durations[name].append(seconds)
    proof = {**touched, 'version': 2, 'tests': identities, 'names': names, 'runs': 5, 'parallelism': 5,
             'retries': 0, 'trials': trials, 'cases': cases, 'durations': durations,
             'load_before': load_before,
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
    parser.add_argument('--red', action='store_true')
    parser.add_argument('args', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    root = Path.cwd().resolve()
    if args.plan:
        print(json.dumps(plan(root, args.base, args.head or 'HEAD')))
    else:
        requested = args.args[1:] if args.args[:1] == ['--'] else args.args
        run_requested(root, args.base, requested, args.cargo, args.verified_script, args.count_file,
                      json.loads(args.target_metadata) if args.target_metadata else None,
                      args.head, args.red)


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, KeyError, subprocess.SubprocessError) as error:
        print('touched-test stability: '+str(error), file=sys.stderr)
        sys.exit(1)
