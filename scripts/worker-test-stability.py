#!/usr/bin/env python3
"""Five concurrent, zero-retry executions of touched Rust tests, in the worker node.

Planning reads immutable Git blobs only. No compiler/test runs occur at park.
The embedded runner supplies the same canonical lexer used by repository tools.
"""
import argparse
import ast
from concurrent.futures import ThreadPoolExecutor
import glob
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import sqlite3
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
    for path in sorted(p for p in changed if p):
        old = git(root, 'show', f'{base}:{path}', missing=True)
        new = git(root, 'show', f'{head}:{path}')
        runner = ecosystem(path, new)
        if runner is None:
            continue
        before = {test['function']: test['fingerprint'] for test in source_tests(path, old, runner, root)}
        for test in source_tests(path, new, runner, root):
            if before.get(test['function']) != test['fingerprint']:
                tests.append({**test, 'runner': runner, 'file': path,
                              'id': path+'::'+test['function']})
    return {'version': 1, 'base': base, 'head': head, 'tests': tests}


def ecosystem(path, source=''):
    suffix = Path(path).suffix.lower()
    name = Path(path).name.lower()
    if suffix == '.rs':
        return 'rust'
    if path.startswith('hub-web/') and suffix in ('.js', '.jsx', '.mjs', '.cjs', '.ts', '.tsx') and re.search(r'(?:\.test|\.spec|\.journey)\.', name):
        if re.search(r"(?:from\s*|import\s*\(|require\s*\()(['\"])node:test\1", source):
            return 'node-test'
        return 'vitest' if '.test.' in name else None
    return None


def source_tests(path, source, runner, root=None):
    if not source:
        return []
    if runner == 'rust':
        return items(source)[0]
    if runner == 'pytest':
        try:
            tree = ast.parse(source, filename=path)
        except SyntaxError as error:
            raise ValueError(f'cannot resolve touched pytest tests in {path}: {error}') from error
        found = []
        def visit(node, scope=()):
            for child in getattr(node, 'body', ()):
                if isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef)):
                    if child.name.startswith('test_') or (scope and scope[0].startswith('Test')):
                        name = '::'.join((*scope, child.name))
                        found.append({'function': name,
                                      'fingerprint': hashlib.sha256(ast.dump(child, include_attributes=False).encode()).hexdigest()})
                elif isinstance(child, ast.ClassDef):
                    if child.name.startswith('Test'):
                        visit(child, (*scope, child.name))
                    elif child.name == 'pytest_generate_tests':
                        raise ValueError(f'unresolved dynamic pytest collection in {path}')
        visit(tree)
        if len({test['function'] for test in found}) != len(found):
            raise ValueError(f'ambiguous pytest test identity in {path}')
        return found
    if root is not None and (root / 'hub-web/node_modules/typescript/lib/typescript.js').is_file():
        parser = root / 'scripts/worker-js-test-source.mjs'
        result = subprocess.run(['node', str(parser), path], cwd=root, input=source,
                                capture_output=True, text=True, timeout=20)
        if result.returncode:
            message = next((line.strip()[7:] for line in result.stderr.splitlines() if line.strip().startswith('Error: ')),
                           result.stderr.splitlines()[0] if result.stderr else f'cannot resolve touched tests in {path}')
            raise ValueError(message)
        try:
            tests = json.loads(result.stdout)
        except json.JSONDecodeError as error:
            raise ValueError(f'invalid JavaScript test plan for {path}') from error
        for test in tests:
            test['fingerprint'] = hashlib.sha256(test['fingerprint'].encode()).hexdigest()
        return tests
    return javascript_tests(path, source)


def javascript_tests(path, source):
    """Extract literal test()/it() calls with balanced source ranges; refuse dynamic titles."""
    tokens, pairs = [], {}
    i = 0
    while i < len(source):
        char = source[i]
        if char.isspace():
            i += 1
        elif source.startswith('//', i):
            end = source.find('\n', i + 2)
            i = len(source) if end < 0 else end + 1
        elif source.startswith('/*', i):
            end = source.find('*/', i + 2)
            if end < 0:
                raise ValueError(f'unclosed comment in {path}')
            i = end + 2
        elif char in ('\"', "'"):
            start, quote = i, char
            i += 1
            while i < len(source):
                if source[i] == '\\':
                    i += 2
                elif source[i] == quote:
                    i += 1
                    break
                else:
                    i += 1
            else:
                raise ValueError(f'unclosed string in {path}')
            raw = source[start:i]
            try:
                value = json.loads(raw) if quote == '\"' else ast.literal_eval(raw)
            except (ValueError, SyntaxError):
                raise ValueError(f'unresolved test title string in {path}')
            tokens.append(('string', str(value), start, i))
        elif char == '`':
            start = i
            i += 1
            value = []
            while i < len(source):
                if source[i] == '\\':
                    if i + 1 < len(source):
                        value.extend((source[i], source[i + 1]))
                    i += 2
                elif source[i] == '`':
                    break
                else:
                    value.append(source[i])
                    i += 1
            if i >= len(source):
                raise ValueError(f'unclosed template string in {path}')
            raw = ''.join(value)
            if '${' in raw:
                tokens.append(('dynamic-string', raw, start, i + 1))
            else:
                tokens.append(('string', raw, start, i + 1))
            i += 1
        elif char == '/' and regex_can_start(tokens):
            start = i
            i += 1
            in_class = False
            while i < len(source):
                if source[i] == '\\':
                    i += 2
                elif source[i] == '[':
                    in_class = True
                    i += 1
                elif source[i] == ']':
                    in_class = False
                    i += 1
                elif source[i] == '/' and not in_class:
                    i += 1
                    while i < len(source) and source[i].isalpha():
                        i += 1
                    break
                elif source[i] in '\r\n':
                    raise ValueError(f'unclosed regular expression in {path}')
                else:
                    i += 1
            else:
                raise ValueError(f'unclosed regular expression in {path}')
            tokens.append(('regex', source[start:i], start, i))
        elif char.isalpha() or char in '_$':
            start = i
            i += 1
            while i < len(source) and (source[i].isalnum() or source[i] in '_$'):
                i += 1
            tokens.append(('id', source[start:i], start, i))
        elif char.isdigit():
            start = i
            i += 1
            while i < len(source) and (source[i].isalnum() or source[i] in '._'):
                i += 1
            tokens.append(('number', source[start:i], start, i))
        else:
            tokens.append(('punct', char, i, i + 1))
            i += 1
    stack = []
    for index, token in enumerate(tokens):
        value = token[1]
        if value in ('(', '[', '{'):
            stack.append((value, index))
        elif value in (')', ']', '}'):
            if not stack or {'(': ')', '[': ']', '{': '}'}[stack[-1][0]] != value:
                raise ValueError(f'unbalanced test source in {path}')
            _, opening = stack.pop()
            pairs[opening] = index
    if stack:
        raise ValueError(f'unbalanced test source in {path}')
    calls = []
    for index, token in enumerate(tokens):
        if token[0] != 'id' or token[1] not in ('test', 'it', 'describe'):
            continue
        if index + 1 < len(tokens) and tokens[index + 1][1] == '.':
            if index + 2 < len(tokens) and tokens[index + 2][1] in ('each', 'only', 'skip', 'concurrent', 'serial', 'fixme'):
                raise ValueError(f'unresolved dynamic {token[1]}.{tokens[index+2][1]} collection in {path}')
            continue
        if index + 1 >= len(tokens) or tokens[index + 1][1] != '(':
            continue
        opening = index + 1
        closing = pairs.get(opening)
        if closing is None:
            raise ValueError(f'unresolved {token[1]} call in {path}')
        if index + 2 >= closing or tokens[index + 2][0] != 'string':
            if token[1] in ('test', 'it'):
                raise ValueError(f'unresolved dynamic test title in {path}')
            continue
        calls.append((token[1], tokens[index + 2][1], index, closing))
    tests, suites = [], []
    for kind, title, start, end in calls:
        if kind == 'describe':
            suites.append((start, end, title))
    for kind, title, start, end in calls:
        if kind not in ('test', 'it'):
            continue
        scopes = [name for suite_start, suite_end, name in suites if suite_start < start < suite_end]
        name = '::'.join((*scopes, title))
        if any(test['function'] == name for test in tests):
            raise ValueError(f'ambiguous test title {name!r} in {path}')
        body = json.dumps([(token[0], token[1]) for token in tokens[start:end+1]], ensure_ascii=False)
        tests.append({'function': name, 'line': source.count('\n', 0, tokens[start][2]) + 1,
                      'fingerprint': hashlib.sha256(body.encode()).hexdigest()})
    return tests


def regex_can_start(tokens):
    """A slash begins a regex where JavaScript permits a new expression."""
    if not tokens:
        return True
    kind, value, *_ = tokens[-1]
    if kind == 'id' and value not in {
        'return', 'throw', 'case', 'delete', 'void', 'typeof', 'new', 'in',
        'of', 'yield', 'await', 'else', 'do', 'instanceof',
    }:
        return False
    if kind in ('string', 'regex', 'number'):
        return False
    if kind == 'punct' and value in (')', ']', '}', '.', '++', '--'):
        return False
    return True


def adapter_args(runner, tests):
    """Produce the narrow native selector for the touched cases in one ecosystem."""
    if not tests or any(test['runner'] != runner for test in tests):
        raise ValueError(f'no resolvable touched {runner} tests')
    if runner == 'pytest':
        raise ValueError('pytest touched-test stability receipts are not supported yet')
    if runner == 'playwright':
        raise ValueError('Playwright touched-test stability receipts are not supported yet')
    if runner == 'vitest':
        files = sorted({test['file'] for test in tests})
        names = [' '.join(test['function'].split('::')) for test in tests]
        pattern = '^(?:' + '|'.join(re.escape(name) for name in names) + ')$'
        return ['--testNamePattern', pattern, *files]
    if runner == 'node-test':
        files = sorted({test['file'] for test in tests})
        names = [test['function'].replace('::', ' ') for test in tests]
        pattern = '^(?:' + '|'.join(re.escape(name) for name in names) + ')$'
        return ['--test-name-pattern', pattern, *files]
    raise ValueError('unsupported stability ecosystem: '+runner)


def worker_context(root):
    """Resolve the active task's recorded merge target for JS/Python wrappers."""
    if os.environ.get('CAS_WORKER_STABILITY_BASE'):
        return {'base': os.environ['CAS_WORKER_STABILITY_BASE'],
                'head': os.environ.get('CAS_WORKER_STABILITY_HEAD') or git(root, 'rev-parse', 'HEAD').strip(),
                'cas_root': Path(os.environ['CAS_ROOT']).resolve() if os.environ.get('CAS_ROOT') else None}
    branch = git(root, 'branch', '--show-current').strip()
    match = re.search(r'(?:^|-)cas-([a-z0-9]+)$', branch)
    if not match:
        if os.environ.get('CAS_FACTORY_MODE') == '1':
            raise ValueError('cannot resolve active factory task from branch; no stability receipt')
        return None
    task_id = 'cas-' + match[1]
    cas_root = os.environ.get('CAS_ROOT')
    if not cas_root:
        raise ValueError('CAS_ROOT missing for active factory task; no stability receipt')
    database = Path(cas_root).resolve() / 'cas.db'
    try:
        connection = sqlite3.connect(f'file:{database}?mode=ro', uri=True)
        try:
            row = connection.execute('SELECT deliverables,status FROM tasks WHERE id=?', (task_id,)).fetchone()
        finally:
            connection.close()
    except sqlite3.Error as error:
        raise ValueError(f'cannot resolve target for {task_id}: {error}; no stability receipt') from error
    if row is None or row[1] != 'in_progress':
        raise ValueError(f'active factory task {task_id} is missing; no stability receipt')
    try:
        target = json.loads(row[0])['work_target']['target_branch']
    except (TypeError, KeyError, json.JSONDecodeError) as error:
        raise ValueError(f'{task_id} has no recorded test target; no stability receipt') from error
    head = git(root, 'rev-parse', 'HEAD').strip()
    remote = 'refs/remotes/origin/' + target
    base = git(root, 'merge-base', remote, head, missing=False).strip()
    return {'base': base, 'head': head, 'cas_root': Path(cas_root).resolve()}


def rust_test_receipt_path(cas_root, root, head, runner):
    package = 'web-tests'
    harness = runner
    filter_name = 'test(worker-stability)'
    repo_key = hashlib.sha256(os.fsencode(str(root.resolve()))).hexdigest()
    identity = json.dumps([package, harness, filter_name], ensure_ascii=False, separators=(',', ':')).encode()
    key = hashlib.sha256(identity).hexdigest()
    return cas_root / 'worker-checks' / repo_key / f'{head}-test-{key}.json', package, harness, filter_name


def report_durations(runner, report, tests):
    durations = {test['id']: [] for test in tests}
    if runner == 'vitest':
        data = json.loads(Path(report).read_text())
        for result in data.get('testResults', []):
            file = Path(result.get('name', '')).as_posix()
            for case in result.get('assertionResults', []):
                full_name = case.get('fullName') or ' '.join(case.get('ancestorTitles', []) + [case.get('title', '')])
                matched = next((test for test in tests if file.endswith(Path(test['file']).as_posix())
                                and ' '.join(test['function'].split('::')) == full_name), None)
                if matched and isinstance(case.get('duration'), (int, float)):
                    durations[matched['id']].append(float(case['duration']) / 1000)
    elif runner == 'node-test':
        import xml.etree.ElementTree as ET
        for case in ET.parse(report).iter('testcase'):
            name = case.attrib.get('name', '')
            matched = next((test for test in tests if test['function'] == name
                            or ' '.join(test['function'].split('::')) == name
                            or name.endswith(' ' + ' '.join(test['function'].split('::')))), None)
            if matched and 'time' in case.attrib:
                durations[matched['id']].append(float(case.attrib['time']))
    else:
        raise ValueError(f'{runner} report parsing is not supported yet')
    if any(not values for values in durations.values()):
        raise ValueError(f'{runner} report does not resolve every touched test')
    return durations


def run_adapter_stability(root, runner, report, initial_args=(), invoke=subprocess.run):
    if os.environ.get('CAS_WORKER_STABILITY_TRIAL') == '1':
        return
    context = worker_context(root)
    if context is None or runner == 'playwright':
        return
    if runner not in ('vitest', 'node-test'):
        raise ValueError(f'{runner} touched-test stability receipts are not supported yet')
    touched = plan(root, context['base'], context['head'])
    tests = [test for test in touched['tests'] if test['runner'] == runner]
    if not tests:
        return
    selectors = adapter_args(runner, tests)
    cas_root = context['cas_root']
    if cas_root is None:
        raise ValueError('CAS_ROOT missing; no bound stability receipt')
    path, package, harness, filter_name = rust_test_receipt_path(cas_root, root, context['head'], runner)
    path.unlink(missing_ok=True)
    (root / 'target').mkdir(parents=True, exist_ok=True)
    directory = Path(tempfile.mkdtemp(prefix='stability-', dir=root / 'target'))
    if runner == 'vitest':
        executable = [sys.executable, str(Path(__file__).resolve()), '--adapter-trial', runner]
        command = [*executable, '--', *selectors, '--retry=0']
    else:
        command = [sys.executable, str(Path(__file__).resolve()), '--adapter-trial', runner, '--', *selectors]
    ready = __import__('threading').Barrier(5)
    def trial(number):
        log = directory / f'run-{number}.log'
        count = directory / f'count-{number}'
        trial_report = directory / f'report-{number}.json'
        env = {**os.environ, 'CAS_WORKER_STABILITY_TRIAL': '1',
               'VERIFIED_TEST_COUNT_FILE': str(count), 'VERIFIED_TEST_REPORT_FILE': str(trial_report)}
        start = time.monotonic()
        with log.open('w') as stream:
            ready.wait()
            result = invoke(command, cwd=root, env=env, stdout=stream,
                            stderr=subprocess.STDOUT, close_fds=False)
        end = time.monotonic()
        observed = int(count.read_text()) if result.returncode == 0 and count.is_file() else 0
        return {'exit_code': result.returncode, 'count': observed, 'started': start, 'ended': end, 'log': str(log)}, trial_report
    from concurrent.futures import ThreadPoolExecutor
    with ThreadPoolExecutor(max_workers=5) as pool:
        results = list(pool.map(trial, range(1, 6)))
    trials = [item[0] for item in results]
    reports = [item[1] for item in results]
    for trial_result in trials:
        print(f"--- touched-test stability copy: {trial_result['log']} ---", flush=True)
        print(Path(trial_result['log']).read_text(errors='replace'), flush=True)
    if any(trial['exit_code'] != 0 for trial in trials):
        raise ValueError(f'{runner} stability failed; original failed copies retained, no retries; park has no receipt')
    if max(trial['started'] for trial in trials) >= min(trial['ended'] for trial in trials):
        raise ValueError(f'{runner} runs did not overlap; no concurrent stability receipt recorded')
    durations = {test['id']: [] for test in tests}
    proof_trials = []
    for trial, report_path in zip(trials, reports):
        run_durations = report_durations(runner, report_path, tests)
        if sum(map(len, run_durations.values())) != trial['count']:
            raise ValueError(f'{runner} report count does not match passed test count; no receipt')
        for ident, values in run_durations.items():
            durations[ident].append(max(values))
        # A Playwright case may run once per configured project. Preserve the
        # actual count for audit while binding the Rust gate to logical tests.
        proof_trials.append({**trial, 'count': len(tests), 'native_count': trial['count']})
    for ident, values in durations.items():
        if len(values) < 5:
            raise ValueError(f'{runner} report lacks five per-test durations for {ident}')
    proof = {**touched, 'version': 2, 'tests': [test['id'] for test in tests],
             'names': [test['id'] for test in tests], 'runs': 5, 'parallelism': 5, 'retries': 0,
             'trials': proof_trials, 'cases': {test['id']: [test['id']] for test in tests},
             'durations': durations, 'load_before': os.getloadavg() if hasattr(os, 'getloadavg') else None,
             'load_after': os.getloadavg() if hasattr(os, 'getloadavg') else None}
    path.parent.mkdir(parents=True, exist_ok=True)
    receipt = {'head': context['head'], 'repo': str(root.resolve()), 'packages': [package],
               'test': {'filter': filter_name, 'harness': harness, 'count': len(tests), 'stability': proof}}
    temporary = path.with_name(path.name + '.tmp-' + str(os.getpid()))
    temporary.write_text(json.dumps(receipt, separators=(',', ':')))
    temporary.replace(path)
    print(f"stability: PASS {context['head']} tests={len(tests)} runs=5 parallelism=5 retries=0 receipt={path}", flush=True)


def adapter_trial(runner, args):
    root = Path.cwd().resolve()
    if runner == 'vitest':
        hub = root / 'hub-web'
        script = hub / 'scripts/run-verified-tests.mjs'
        selectors = [arg.removeprefix('hub-web/') if arg.startswith('hub-web/') else arg for arg in args]
        command = [shutil.which('node') or 'node', str(script), runner, *selectors]
    elif runner == 'node-test':
        selectors = [arg.removeprefix('hub-web/') if arg.startswith('hub-web/') else arg for arg in args]
        command = [shutil.which('node') or 'node', str(root / 'hub-web/scripts/run-verified-node-tests.mjs'), *selectors]
    else:
        raise ValueError(f'{runner} touched-test stability receipts are not supported yet')
    result = subprocess.run(command, cwd=(root / 'hub-web' if runner == 'vitest' else root), close_fds=False)
    return result.returncode


def option_args(args, options):
    """Retain configuration that affects collection while preserving paired values."""
    kept, index = [], 0
    while index < len(args):
        arg = args[index]
        option = next((candidate for candidate in options
                       if arg == candidate or arg.startswith(candidate + '=')), None)
        if option:
            kept.append(arg)
            if arg == option and index + 1 < len(args):
                kept.append(args[index + 1])
                index += 1
        index += 1
    return kept


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
    parser.add_argument('--base')
    parser.add_argument('--head')
    parser.add_argument('--run-adapter', choices=('vitest', 'node-test', 'playwright', 'pytest'))
    parser.add_argument('--adapter-trial', choices=('vitest', 'node-test', 'playwright', 'pytest'))
    parser.add_argument('--report')
    parser.add_argument('--cargo')
    parser.add_argument('--verified-script')
    parser.add_argument('--count-file')
    parser.add_argument('--target-metadata', default=os.environ.get('CAS_WORKER_STABILITY_TARGET'))
    parser.add_argument('--red', action='store_true')
    parser.add_argument('args', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    root = Path.cwd().resolve()
    if args.adapter_trial:
        sys.exit(adapter_trial(args.adapter_trial, args.args[1:] if args.args[:1] == ['--'] else args.args))
    if args.run_adapter:
        if os.environ.get('CAS_WORKER_STABILITY_TRIAL') == '1':
            return
        run_adapter_stability(root, args.run_adapter, args.report,
                              args.args[1:] if args.args[:1] == ['--'] else args.args)
    elif args.plan:
        if not args.base:
            parser.error('--base is required with --plan')
        print(json.dumps(plan(root, args.base, args.head or 'HEAD')))
    else:
        if not args.base:
            parser.error('--base is required for Rust stability runs')
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
