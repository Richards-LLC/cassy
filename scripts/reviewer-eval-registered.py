#!/usr/bin/env python3
"""Bridge the eval runner to real, already-registered CAS actors.

Actor adapters consume JSON on stdin and return decoded CAS JSON on stdout.
They supply their own authenticated sessions; this bridge never creates role
credentials. See the binding contract in docs/review/eval/README.md.
"""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys

spec = importlib.util.spec_from_file_location('evaluation', Path(__file__).with_name('reviewer-eval.py'))
evaluation = importlib.util.module_from_spec(spec)
spec.loader.exec_module(evaluation)


def call(binding, actor, operation, payload, out):
    argv = binding['actors'][actor][operation]
    result = subprocess.run(argv, input=json.dumps(payload), text=True,
                            capture_output=True, timeout=1800)
    if result.returncode:
        raise ValueError(f'{actor}/{operation} adapter failed (exit {result.returncode})')
    value = json.loads(result.stdout)
    # Store protocol requests and decoded receipts, never credential-bearing argv/env.
    ledger = out/'protocol.jsonl'
    with ledger.open('a') as file:
        file.write(json.dumps({'actor': actor, 'operation': operation,
                               'request': payload, 'response': value})+'\n')
    return value


def shadow(binding, actor, request, out):
    return call(binding, actor, 'shadow', {'action': 'shadow', 'review': request}, out)


def normalized_rules(rules):
    return sorted(({key: rule[key] for key in ('id', 'paths', 'content')} for rule in rules),
                  key=lambda rule: rule['id'])


def validate_context(axis, context, request, binding):
    expected = request['contexts'][axis]
    if context['axis'] != axis:
        raise ValueError('Wrong authenticated reviewer context')
    if (context['base_commit'], context['head_commit']) != (binding['base_sha'], binding['head_sha']):
        raise ValueError('Registered proof bounds differ from binding')
    sources = context['sources']
    if axis == 'spec':
        if sources['task_description'] != expected['task_description'] or sources['criteria'] != expected['criteria']:
            raise ValueError('Registered task sources differ from corpus')
    else:
        if sources['coding_standards'] != request['sources']['coding_standards']:
            raise ValueError('Registered Standards snapshot differs from pinned source')
        if normalized_rules(sources['promoted_rules']) != normalized_rules(request['sources']['promoted_rules']):
            raise ValueError('Registered promoted-rule snapshot differs from pin')
    # A replay may transplant the pinned standards into BOTH trees. Preserve all
    # original delivery changes byte-for-byte; the transplant adds no diff hunk.
    original = evaluation.git('diff', '--binary', expected['base_commit'], expected['head_commit'])
    replay = evaluation.git('diff', '--binary', binding['base_sha'], binding['head_sha'], cwd=context['worktree'])
    if original != replay:
        raise ValueError('Registered replay does not preserve authentic delivery diff')


def main():
    request = json.load(sys.stdin)
    bindings = json.loads(Path(os.environ['REVIEW_EVAL_BINDINGS']).read_text())
    binding = bindings[request['case_id']]
    actors = ['supervisor', 'spec', 'standards', 'baseline', 'implementer']
    identities = [binding['agent_ids'][actor] for actor in actors]
    if len(set(identities)) != len(identities):
        raise ValueError('Supervisor, reviewers, baseline and implementer must be distinct registered actors')
    out = Path(request['out'])
    out.mkdir(parents=True, exist_ok=True)
    dispatch = binding['dispatch_id']
    start = shadow(binding, 'supervisor', {'op': 'start', 'task_id': binding['task_id'],
                    'dispatch_id': dispatch, 'base_ref': binding['base_sha'],
                    'spec_agent_id': binding['agent_ids']['spec'],
                    'standards_agent_id': binding['agent_ids']['standards']}, out)
    round_id = start['round']['id']
    if start['merge_gate_authority'] is not False or start['round']['dispatch_id'] != dispatch:
        raise ValueError('Not an advisory, dispatch-bound registered round')
    if request['operation'] == 'run':
        for axis in ('spec', 'standards'):
            context = shadow(binding, axis, {'op': 'context', 'round_id': round_id}, out)
            validate_context(axis, context, request, binding)
            result = call(binding, axis, 'model', {'operation': 'review', 'axis': axis,
                          'context': context, 'reference': request['sources'][axis],
                          'schema': evaluation.schema(axis),
                          'no_rust_compilation': True, 'apply': False}, out)
            report, telemetry = result['report'], result['telemetry']
            replay_case = {'head_sha': binding['head_sha'], 'task_context': {
                         'description': request['contexts']['spec']['task_description'],
                         'acceptance_criteria': '\n'.join(request['contexts']['spec']['criteria'])}}
            commits = evaluation.validate_report(report, replay_case, axis, Path(context['worktree']))
            seal = shadow(binding, axis, {'op': 'report', 'round_id': round_id, 'report': report}, out)
            if seal['report'] != report:
                raise ValueError('Registered seal differs from reviewer report')
            evaluation.write(out/axis/'result.json', {'case_id': request['case_id'], 'axis': axis,
                 'report': report, 'telemetry': dict(telemetry, authority='registered'),
                 'commits': commits, 'validation_error': None, 'checkout': context['worktree'],
                 'head_after': seal['reported_tip']})
        baseline = call(binding, 'baseline', 'model', {'operation': 'baseline',
                       'dispatch_id': dispatch, 'body': request['sources']['baseline'],
                       'context': request['contexts']['baseline'], 'no_rust_compilation': True}, out)
        if not baseline.get('verification_receipt'):
            raise ValueError('Registered baseline requires a real bound verification receipt')
        evaluation.write(out/'baseline'/'result.json', {'case_id': request['case_id'], 'axis': 'baseline',
                         'report': baseline['report'], 'telemetry': dict(baseline['telemetry'], authority='registered'),
                         'commits': [], 'validation_error': None,
                         'verification_receipt': baseline['verification_receipt']})
    elif request['operation'] == 'cross-check':
        for axis in ('spec', 'standards'):
            context = shadow(binding, axis, {'op': 'context', 'round_id': round_id, 'cross_check': True}, out)
            other = context['review_other']
            result = call(binding, axis, 'model', {'operation': 'cross-check', 'axis': axis,
                          'context': context, 'own_sources': request['contexts'][axis],
                          'reference': request['sources'][axis], 'schema': evaluation.schema(axis, True),
                          'no_rust_compilation': True, 'apply': False}, out)
            expected = [f['commit'] for f in evaluation.findings(other['report']) if f.get('commit')]
            if sorted(d['commit'] for d in result['decisions']) != sorted(expected):
                raise ValueError('Cross-check must cover every other-axis fix exactly once')
            reverts = []
            for decision in result['decisions']:
                receipt = shadow(binding, axis, dict(op='cross_check', round_id=round_id, **decision), out)
                evaluation.write(out/axis/'cross-check'/f'{decision["commit"]}.receipt.json', receipt)
            evaluation.write(out/axis/'cross-check'/'result.json', dict(result, reverts=reverts,
                                                                       authority='registered', error=None))
    comparison = shadow(binding, 'supervisor', {'op': 'show', 'round_id': round_id}, out)
    if comparison.get('legacy_verdict') is None:
        raise ValueError('Registered comparison lacks the legacy verdict')
    evaluation.write(out/'comparison.json', comparison)
    print(json.dumps({'authority': 'registered', 'round_id': round_id,
                      'dispatch_id': dispatch, 'comparison': str(out/'comparison.json')}))


if __name__ == '__main__':
    main()
