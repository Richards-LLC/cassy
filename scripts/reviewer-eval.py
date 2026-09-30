#!/usr/bin/env python3
"""Independent historical replay. Truth stays outside reviewer checkouts/prompts.

This measures models; it does not grant CAS authority or enable merge policy.
Registered mode delegates to real, pre-registered actors using the same evidence
format (see docs/review/eval/README.md). No runtime identity is synthesized.
"""
import argparse
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import threading
import time
import tomllib

ROOT = Path(__file__).resolve().parents[1]
AXES = ('spec', 'standards', 'baseline')
REF = 'cas-cli/src/builtins/skills/cas-shadow-review/references/'
BODY = 'cas-cli/src/builtins/agents/task-verifier.body.md'


def run(argv, cwd=ROOT, **kwargs):
    return subprocess.check_output(argv, cwd=cwd, text=True, **kwargs).strip()


def git(*argv, cwd=ROOT):
    return run(['git', *argv], cwd=cwd)


def write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + '\n')


def digest(value):
    return hashlib.sha256(value.encode()).hexdigest()


def snapshot(sha, rules_path):
    sha = git('rev-parse', sha)
    def read(path):
        return subprocess.check_output(['git', 'show', f'{sha}:{path}'], cwd=ROOT, text=True)
    rules = json.loads(rules_path.read_text())
    if any(rule['status'] != 'proven' for rule in rules):
        raise ValueError('Only actual promoted rules may be loaded')
    present = subprocess.run(['git', 'cat-file', '-e', f'{sha}:CODING_STANDARDS.md'],
                             cwd=ROOT, capture_output=True).returncode == 0
    result = {'sha': sha, 'spec': read(REF + 'spec.md'),
              'standards': read(REF + 'standards.md'), 'baseline': read(BODY),
              'coding_standards': read('CODING_STANDARDS.md') if present else None,
              'promoted_rules': rules}
    result['sha256'] = digest(json.dumps(result, sort_keys=True))
    return result


def task_context(case):
    task = case['task_context']
    return {'task_description': task['description'],
            'criteria': [line.strip() for line in task['acceptance_criteria'].splitlines() if line.strip()]}


def context(case, axis, sources):
    public = {'base_commit': case['base_sha'], 'head_commit': case['head_sha'],
              'scope': case['scope'], 'axis': axis}
    if axis in ('spec', 'baseline'):
        public.update(task_context(case))
    if axis in ('standards', 'baseline'):
        public.update({key: sources[key] for key in ('coding_standards', 'promoted_rules')})
    return public


def schema(axis, cross=False, public=None):
    def obj(properties):
        return {'type': 'object', 'properties': properties,
                'required': list(properties), 'additionalProperties': False}
    def array(item):
        return {'type': 'array', 'items': item}
    string = {'type': 'string'}
    if cross:
        return obj({'decisions': array(obj({'commit': string,
                    'decision': {'enum': ['accept', 'revert']}, 'reason': string}))})
    source = string
    if public and axis == 'spec':
        source = {'enum': public['criteria']}
    elif public and axis == 'standards':
        allowed = [rule['id'] for rule in public['promoted_rules']]
        if public['coding_standards'] is not None:
            allowed.append('CODING_STANDARDS.md')
        if allowed:
            source = {'enum': allowed}
    finding = obj({'id': string, 'rank': {'type': 'integer'}, 'source': source,
                   'evidence': string, 'uncertain': {'type': 'boolean'},
                   'judgement': {'type': 'boolean'}, 'commit': {'type': ['string', 'null']}})
    return obj({'axis': {'enum': [axis]},
                'status': {'enum': ['approved', 'rejected', 'error', 'skipped']},
                'summary': string,
                'criteria': array(obj({'criterion': string, 'status': string, 'evidence': string})),
                'scope_creep': array(finding), 'findings': array(finding)})


ADAPTER = '''
Isolated eval adapter: API authority and protocol are SIMULATED. This is an
independent replay, not a live CAS task. Do not call CAS, register identities,
send messages, access the network, push, or run Rust builds/tests (cargo, rustc,
nextest, run-scoped-tests.sh, make test). Do not inspect any path outside this
checkout, including evaluator artifacts or other reviewers' checkouts. Do not
seek later commits or grading truth. Only HEAD and its parent are available.
Inspect the specified fixed diff and full files/dependencies needed to explain
it. Findings must name concrete pre-fix file:line and consequences. Distinguish
missing execution evidence from a demonstrated defect. Do not call missing
proof a code defect. No generic all-files audit outside the provided scope.
The historical task text is the stored source; do not change its criteria.
The Standards source is the supplied pinned snapshot, even when that file is
absent in this historical checkout. Do not replace it with invented standards.
Return the supplied JSON shape. Use null commit when no fix is made.
'''


BRIDGE_INSTRUCTIONS = '''
Git commit execution uses a simulated runner bridge because the sandbox protects
.git. Keep the sandbox. Make the fix in tracked files, then write one request to
.review-bridge/requests/<finding-id>.json with exactly:
{"finding_id":"<finding-id>","files":["relative/file"],"fix":"short fix summary"}
One finding per request/commit; list every file belonging to that fix. Read
.review-bridge/responses/<finding-id>.json for the actual full commit SHA. A
rejected request creates no commit; report that fact. Never modify .git or the
bridge, never call git add/commit, and do not combine independent findings in a
single commit. Include the bridge-returned SHA in the typed finding. Bridge
requests are ignored by Git; leave no other uncommitted changes.
'''


def prompt(case, axis, sources, fix_transport='native'):
    body = sources[axis]
    extra = ('\nBaseline adapter: the required verification record is simulated by your final JSON; '
             'do not call verification.add. Remain read-only and never commit. Map substantive '
             'defects into findings; preserve the legacy body\'s review procedure.\n'
             if axis == 'baseline' else
             '\nFor certain own-axis findings, make separate small commits on this side ref only, '
             f'with subject review({axis}): <finding-id> <fix>. Do not broaden the scope. '
             'Never fix another axis\'s concern. Uncertain findings have no commits.\n')
    if fix_transport == 'bridge' and axis != 'baseline':
        extra += BRIDGE_INSTRUCTIONS
    return body + '\n' + ADAPTER + extra + '\nContext:\n' + json.dumps(context(case, axis, sources), indent=2)


class CommitBridge:
    """Only stages requested replay files; no shell, credentials or authority."""
    def __init__(self, checkout, axis, directory):
        self.checkout, self.axis, self.directory = checkout, axis, directory
        self.root = checkout/'.review-bridge'
        (self.root/'requests').mkdir(parents=True)
        (self.root/'responses').mkdir()
        with (checkout/'.git/info/exclude').open('a') as file:
            file.write('\n/.review-bridge/\n')
        self.stop = threading.Event()
        self.thread = threading.Thread(target=self.watch, daemon=True)

    def commit(self, request):
        if not isinstance(request, dict) or set(request) != {'finding_id', 'files', 'fix'}:
            raise ValueError('Unknown commit bridge fields')
        identifier, paths = request['finding_id'], request['files']
        if not identifier or any(c not in 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-' for c in identifier):
            raise ValueError('Invalid finding id')
        if not isinstance(paths, list) or not paths or len(paths) != len(set(paths)):
            raise ValueError('Expected nonempty unique file list')
        for path in paths:
            if not isinstance(path, str) or not path:
                raise ValueError('Expected relative file names')
            relative = Path(path)
            if relative.is_absolute() or any(part in ('.git', '.review-bridge', '..') for part in relative.parts):
                raise ValueError('Forbidden bridge path')
            resolved = (self.checkout/relative).resolve()
            if not resolved.is_relative_to(self.checkout.resolve()):
                raise ValueError('Bridge path escapes checkout')
            if resolved.exists() and not resolved.is_file():
                raise ValueError('Bridge accepts files only')
        fix = request['fix']
        if not isinstance(fix, str) or not fix.strip() or '\n' in fix or len(fix) > 160:
            raise ValueError('Invalid fix summary')
        if git('diff', '--cached', '--name-only', cwd=self.checkout):
            raise ValueError('Unexpected pre-staged changes')
        git('add', '--', *paths, cwd=self.checkout)
        if not git('diff', '--cached', '--name-only', cwd=self.checkout):
            raise ValueError('No staged fix')
        git('commit', '-qm', f'review({self.axis}): {identifier} {fix}', cwd=self.checkout)
        return git('rev-parse', 'HEAD', cwd=self.checkout)

    def watch(self):
        while True:
            for path in sorted((self.root/'requests').glob('*.json')):
                response = self.root/'responses'/path.name
                if response.exists():
                    continue
                try:
                    request = json.loads(path.read_text())
                except json.JSONDecodeError:
                    continue  # Writer may not have finished its atomic rename yet.
                try:
                    if not isinstance(request, dict) or request.get('finding_id') != path.stem:
                        raise ValueError('Filename must bind finding id')
                    result = {'commit': self.commit(request), 'error': None,
                              'commit_execution': 'simulated bridge'}
                except (TypeError, ValueError, subprocess.CalledProcessError) as exc:
                    result = {'commit': None, 'error': str(exc)}
                    git('reset', '-q', cwd=self.checkout)
                write(self.directory/'commit-bridge'/path.name, {'request': request, 'response': result})
                write(response, result)
            if self.stop.wait(0.25):
                break

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *unused):
        self.stop.set()
        self.thread.join()


def clone(case, axis, out):
    checkout = out / case['id'] / axis / 'checkout'
    if checkout.exists():
        raise ValueError(f'Refuse to overwrite replay checkout: {checkout}')
    checkout.mkdir(parents=True)
    git('init', '-q', cwd=checkout)
    # Fetch only two commits: no expected repair commit or evaluator files.
    git('fetch', '-q', '--depth=2', str(ROOT), case['head_sha'], cwd=checkout)
    git('checkout', '-q', '-b', f'review/{case["id"]}/{axis}', 'FETCH_HEAD', cwd=checkout)
    git('config', 'user.name', 'Independent replay reviewer', cwd=checkout)
    git('config', 'user.email', 'reviewer-eval@invalid.local', cwd=checkout)
    # FETCH_HEAD records only the selected historical delivery; no origin remote.
    if git('rev-parse', 'HEAD^', cwd=checkout) != case['base_sha']:
        raise ValueError('Base/head mismatch')
    return checkout


def invoke(checkout, directory, text, axis, timeout, cross=False, public=None):
    directory.mkdir(parents=True, exist_ok=True)
    (directory / 'prompt.txt').write_text(text)
    write(directory / 'schema.json', schema(axis, cross, public))
    config_path = Path(os.environ.get('CODEX_HOME', str(Path.home()/'.codex')))/'config.toml'
    config = tomllib.loads(config_path.read_text()) if config_path.exists() else {}
    argv = ['codex', 'exec', '--ephemeral', '--json', '-s',
            'read-only' if axis == 'baseline' or cross else 'workspace-write',
            '-C', str(checkout), '-c', 'project_doc_max_bytes=0']
    # Disable actual configured servers; creating a nonexistent stanza is invalid.
    for name in config.get('mcp_servers', {}):
        argv += ['-c', f'mcp_servers.{name}.enabled=false']
    argv += [
            '--output-schema', str(directory / 'schema.json'),
            '-o', str(directory / 'report.json'), '-']
    env = {key: value for key, value in os.environ.items()
           if not key.startswith(('CAS_', 'CASSY_', 'CLAUDE_'))}
    # No model override: every axis uses the same configured default.
    start = time.monotonic()
    rc, error = None, None
    with (directory / 'events.jsonl').open('w') as events, (directory / 'stderr.log').open('w') as err:
        process = subprocess.Popen(argv, stdin=subprocess.PIPE, text=True, env=env,
                                   stdout=events, stderr=err, start_new_session=True)
        try:
            process.communicate(text, timeout=timeout)
            rc = process.returncode
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGTERM)
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
            error = 'timeout'
    usage, models = [], []
    for line in (directory / 'events.jsonl').read_text().splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if 'usage' in event:
            usage.append(event['usage'])
        if 'model' in event:
            models.append(event['model'])
    telemetry = {'exit_code': rc, 'error': error, 'elapsed_seconds': time.monotonic()-start,
                 'usage': usage, 'models_observed': models, 'argv': argv,
                 'configured_model': config.get('model'),
                 'configured_reasoning_effort': config.get('model_reasoning_effort'),
                 'prompt_sha256': digest(text), 'authority': 'simulated'}
    report = None
    if rc == 0:
        try:
            report = json.loads((directory / 'report.json').read_text())
        except (ValueError, FileNotFoundError) as exc:
            telemetry['error'] = str(exc)
    write(directory / 'telemetry.json', telemetry)
    return report, telemetry


def findings(report):
    return report.get('findings', []) + report.get('scope_creep', [])


def validate_report(report, case, axis, checkout):
    if report is None or report.get('axis') != axis:
        raise ValueError('Missing/incorrect report axis')
    if report['status'] not in ('approved', 'rejected', 'error', 'skipped'):
        raise ValueError('Unknown report status')
    if axis == 'standards' and (report['criteria'] or report['scope_creep']):
        raise ValueError('Standards report contains Spec material')
    if axis == 'spec' and [v['criterion'] for v in report['criteria']] != task_context(case)['criteria']:
        raise ValueError('Spec criteria must quote every exact source line in order')
    if axis == 'spec' and any(f['source'] not in task_context(case)['criteria'] for f in findings(report)):
        raise ValueError('Spec finding must quote its exact source criterion')
    ids = [f['id'] for f in findings(report)]
    if len(ids) != len(set(ids)):
        raise ValueError('Finding identifiers must be unique')
    if report['status'] == 'approved' and any(v['status'] != 'approved' for v in report['criteria']):
        raise ValueError('Approved report with unapproved criterion')
    commits = git('rev-list', '--reverse', f'{case["head_sha"]}..HEAD', cwd=checkout).splitlines()
    declared = [f['commit'] for f in findings(report) if f.get('commit')]
    if len(declared) != len(set(declared)) or set(commits) != set(declared):
        raise ValueError('Every actual fix commit must belong to exactly one finding')
    if axis == 'baseline' and commits:
        raise ValueError('Baseline must not change delivery')
    if git('status', '--porcelain', cwd=checkout):
        raise ValueError('Reviewer left uncommitted changes')
    for finding in findings(report):
        commit = finding.get('commit')
        if commit:
            if finding['uncertain']:
                raise ValueError('Uncertain finding has a fix')
            subject = git('show', '-s', '--format=%s', commit, cwd=checkout)
            if not subject.startswith(f'review({axis}): {finding["id"]} '):
                raise ValueError('Fix subject does not bind axis and finding')
    return commits


def review(case, axis, sources, out, timeout, fix_transport='native'):
    directory = out / case['id'] / axis
    if (directory / 'result.json').exists():
        return json.loads((directory / 'result.json').read_text())
    checkout = clone(case, axis, out)
    text = prompt(case, axis, sources, fix_transport)
    public = context(case, axis, sources)
    if fix_transport == 'bridge' and axis != 'baseline':
        with CommitBridge(checkout, axis, directory):
            report, telemetry = invoke(checkout, directory, text, axis, timeout, public=public)
    else:
        report, telemetry = invoke(checkout, directory, text, axis, timeout, public=public)
    telemetry['commit_execution'] = fix_transport if axis != 'baseline' else 'none'
    error, commits = None, []
    try:
        commits = validate_report(report, case, axis, checkout)
    except (ValueError, KeyError) as exc:
        error = str(exc)
        commits = git('rev-list', '--reverse', f'{case["head_sha"]}..HEAD', cwd=checkout).splitlines()
    result = {'case_id': case['id'], 'axis': axis, 'report': report,
              'telemetry': telemetry, 'validation_error': error, 'commits': commits,
              'checkout': str(checkout), 'head_after': git('rev-parse', 'HEAD', cwd=checkout)}
    write(directory / 'result.json', result)
    return result


def cross_check(case, checker, sources, out, timeout):
    owner = 'standards' if checker == 'spec' else 'spec'
    result = json.loads((out / case['id'] / owner / 'result.json').read_text())
    own = json.loads((out / case['id'] / checker / 'result.json').read_text())
    directory = out / case['id'] / checker / 'cross-check'
    if (directory / 'result.json').exists():
        return
    if result['validation_error'] or own['validation_error']:
        write(directory / 'result.json', {'error': 'Independent report invalid; cross-check pending'})
        return
    fixes = []
    for commit in result['commits']:
        fixes.append({'commit': commit, 'patch': git('show', '--format=fuller', commit, cwd=result['checkout'])})
    if not fixes:
        write(directory / 'result.json', {'decisions': [], 'telemetry': None, 'reverts': []})
        return
    text = sources[checker] + '\n' + ADAPTER + '\nCross-check phase. Both independent reports are sealed. '
    text += 'Read the other axis\'s exact fix patches below, judging only their effects on your axis sources. '
    text += 'Remain read-only. Accept or request revert for EACH commit with concrete evidence. '
    text += '\nOwn context:\n' + json.dumps(context(case, checker, sources))
    text += '\nOther fixes:\n' + json.dumps(fixes)
    report, telemetry = invoke(Path(own['checkout']), directory, text, checker, timeout, cross=True)
    decisions = report['decisions'] if report else []
    expected = result['commits']
    error = None
    if sorted(d['commit'] for d in decisions) != sorted(expected):
        error = 'Missing, duplicate or unknown cross-check decision'
    reverts = []
    if not error:
        for decision in reversed(decisions):
            if decision['decision'] == 'revert':
                try:
                    git('revert', '--no-commit', decision['commit'], cwd=result['checkout'])
                    git('commit', '-qm', f'review({checker}) cross-check revert: {decision["commit"]} {decision["reason"]}', cwd=result['checkout'])
                    reverts.append({'fix': decision['commit'], 'revert': git('rev-parse', 'HEAD', cwd=result['checkout'])})
                except subprocess.CalledProcessError as exc:
                    error = f'Revert failed: {exc}'
                    break
    write(directory / 'result.json', {'decisions': decisions, 'telemetry': telemetry,
                                      'error': error, 'reverts': reverts})


def score(corpus, out, labels):
    """No text-matching oracle. Independent adjudication is required per finding."""
    results = []
    for case in corpus['cases']:
        for axis in AXES:
            path = out / case['id'] / axis / 'result.json'
            result = json.loads(path.read_text()) if path.exists() else {'validation_error': 'Missing run'}
            result.update(case_id=case['id'], axis=axis)
            results.append(result)
    rows, pending, all_detected = [], [], set()
    for axis in AXES:
        selected = [r for r in results if r['axis'] == axis]
        expected = [c for c in corpus['cases'] if c['kind'] == 'defect' and
                    (axis == 'baseline' or c['expected_axis'] == axis)]
        detected, tp, fp, subjective = set(), 0, 0, 0
        commits, bad, test_unknown = set(), set(), set()
        input_tokens, cached_tokens, output_tokens, elapsed, errors = 0, 0, 0, 0, 0
        cross_input, cross_output, cross_time = 0, 0, 0
        telemetry_missing = 0
        clean_flagged = set()
        clean_ids = {c['id'] for c in corpus['cases'] if c['kind'] == 'clean'}
        for result in selected:
            if result.get('validation_error'):
                errors += 1
            for finding in findings(result.get('report') or {}):
                key = f'{result["case_id"]}/{axis}/{finding["id"]}'
                label = labels.get('findings', {}).get(key)
                if label is None:
                    pending.append(key)
                    continue
                if finding.get('judgement'):
                    subjective += 1
                if label['correct']:
                    tp += 1
                    if label['seed_match']:
                        detected.add(result['case_id'])
                        if axis != 'baseline':
                            all_detected.add(result['case_id'])
                else:
                    fp += 1
                    if result['case_id'] in clean_ids:
                        clean_flagged.add(result['case_id'])
            for commit in result.get('commits', []):
                commits.add(commit)
                label = labels.get('commits', {}).get(commit)
                if not label or label.get('targeted_test_status') not in ('pass', 'fail'):
                    test_unknown.add(commit)
                if label and label.get('targeted_test_status') == 'fail':
                    bad.add(commit)
                other = 'standards' if axis == 'spec' else 'spec'
                cross_path = out / result['case_id'] / other / 'cross-check' / 'result.json'
                if axis != 'baseline' and cross_path.exists():
                    cross = json.loads(cross_path.read_text())
                    if cross.get('error') or not any(d['commit'] == commit for d in cross.get('decisions', [])):
                        pending.append(f'{result["case_id"]}/{axis}/cross-check/{commit}')
                    if any(d['commit'] == commit and d['decision'] == 'revert' for d in cross.get('decisions', [])):
                        bad.add(commit)
                elif axis != 'baseline':
                    pending.append(f'{result["case_id"]}/{axis}/cross-check/{commit}')
            telemetry = result.get('telemetry') or {}
            if not telemetry.get('usage'):
                telemetry_missing += 1
            elapsed += telemetry.get('elapsed_seconds', 0)
            for usage in telemetry.get('usage', []):
                input_tokens += usage.get('input_tokens', 0)
                cached_tokens += usage.get('cached_input_tokens', 0)
                output_tokens += usage.get('output_tokens', 0)
            cross_path = out/result['case_id']/axis/'cross-check'/'result.json'
            if cross_path.exists():
                cross = json.loads(cross_path.read_text()).get('telemetry') or {}
                cross_time += cross.get('elapsed_seconds', 0)
                for usage in cross.get('usage', []):
                    cross_input += usage.get('input_tokens', 0)
                    cross_output += usage.get('output_tokens', 0)
        expected_ids = {c['id'] for c in expected}
        hits = detected & expected_ids
        groups = {c['defect_group'] for c in expected}
        hit_groups = {c['defect_group'] for c in expected if c['id'] in hits}
        rows.append({'axis': axis, 'seed_hits': len(hits), 'seed_total': len(expected),
                     'recall': len(hits)/len(expected) if expected else None,
                     'grouped_hits': len(hit_groups), 'grouped_total': len(groups),
                     'grouped_recall': len(hit_groups)/len(groups) if groups else None,
                     'true_findings': tp, 'false_findings': fp,
                     'precision': tp/(tp+fp) if tp+fp else None,
                     'judgement_findings': subjective, 'fix_commits': len(commits),
                     'clean_negatives_false_flagged': len(clean_flagged), 'clean_negatives_total': len(clean_ids),
                     'bad_commits': len(bad), 'bad_commit_rate_lower_bound': len(bad)/len(commits) if commits else None,
                     'targeted_tests_unknown': len(test_unknown), 'input_tokens': input_tokens,
                     'cached_input_tokens': cached_tokens, 'output_tokens': output_tokens,
                     'elapsed_seconds': elapsed, 'process_errors': errors,
                     'token_telemetry_missing_runs': telemetry_missing,
                     'cross_check_input_tokens': cross_input, 'cross_check_output_tokens': cross_output,
                     'cross_check_elapsed_seconds': cross_time})
    all_seeds = {c['id'] for c in corpus['cases'] if c['kind'] == 'defect'}
    combined_hits = all_detected & all_seeds
    return {'rows': rows, 'pending_adjudication': pending,
            'combined_seed_hits': len(combined_hits), 'combined_seed_total': len(all_seeds),
            'combined_recall': len(combined_hits)/len(all_seeds) if all_seeds else None,
            'policy_ready': False, 'authority': 'simulated unless registered receipts independently verified',
            'mandatory_preconditions': ['Operator approves thresholds',
                 'Installed 3.39.0 registered shadow protocol rerun', 'Supervisor targeted-test proof for all fix commits']}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('operation', choices=['run', 'cross-check', 'score'])
    parser.add_argument('--corpus', type=Path, default=ROOT/'docs/review/eval/corpus.json')
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--rules', type=Path)
    parser.add_argument('--reviewer-sha', default='HEAD')
    parser.add_argument('--cases', help='Comma-separated opaque case IDs; omitted means all')
    parser.add_argument('--axes', default='spec,standards,baseline', help='Independent axes to run; useful for staged execution')
    parser.add_argument('--jobs', type=int, default=2)
    parser.add_argument('--timeout', type=int, default=600)
    parser.add_argument('--fix-transport', choices=['native', 'bridge'], default='native',
                        help='Bridge mechanically commits reviewer-selected files, preserving sandbox limits')
    parser.add_argument('--labels', type=Path)
    parser.add_argument('--registered-adapter', type=Path,
                        help='Registered supervisor/children executable; JSON stdin, evidence JSON stdout')
    args = parser.parse_args()
    corpus = json.loads(args.corpus.read_text())
    axes = args.axes.split(',')
    if any(axis not in AXES for axis in axes) or len(axes) != len(set(axes)):
        parser.error('--axes must contain distinct spec,standards,baseline values')
    cases = [c for c in corpus['cases'] if not args.cases or c['id'] in args.cases.split(',')]
    out = args.out.resolve()
    if args.operation == 'score':
        labels = json.loads(args.labels.read_text()) if args.labels else {}
        write(out/'metrics.json', score(corpus, out, labels))
        print(json.dumps(json.loads((out/'metrics.json').read_text()), indent=2))
        return
    if not args.rules:
        parser.error('--rules is required')
    sources = snapshot(args.reviewer_sha, args.rules)
    pin = out/'sources.json'
    if pin.exists() and json.loads(pin.read_text()) != sources:
        raise ValueError('Source snapshot changed; use a new output directory')
    write(pin, sources)
    if args.registered_adapter:
        # The adapter runs real registered actors, never aliases this worker as a supervisor.
        for case in cases:
            request = {'operation': args.operation, 'case_id': case['id'], 'sources': sources,
                       'contexts': {axis: context(case, axis, sources) for axis in AXES},
                       'out': str(out/case['id'])}
            receipt = json.loads(run([str(args.registered_adapter.resolve())],
                                     input=json.dumps(request)))
            if receipt.get('authority') != 'registered' or not receipt.get('round_id') or not receipt.get('dispatch_id'):
                raise ValueError('Adapter did not return registered round/dispatch receipts')
            write(out/case['id']/'registered-receipt.json', receipt)
        return
    operations = [(case, axis) for case in cases for axis in
                  (axes if args.operation == 'run' else ('spec', 'standards'))]
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as executor:
        action = review if args.operation == 'run' else cross_check
        futures = {executor.submit(action, case, axis, sources, out, args.timeout,
                     *([args.fix_transport] if args.operation == 'run' else [])): (case['id'], axis)
                   for case, axis in operations}
        for future in concurrent.futures.as_completed(futures):
            name = futures[future]
            try:
                result = future.result()
                print(json.dumps({'case': name[0], 'axis': name[1],
                                  'validation_error': result.get('validation_error') if result else None}), flush=True)
            except Exception as exc:
                print(json.dumps({'case': name[0], 'axis': name[1], 'error': str(exc)}), flush=True)
    print('Complete; process errors and missing results remain in the scoring denominator.', flush=True)


if __name__ == '__main__':
    main()
