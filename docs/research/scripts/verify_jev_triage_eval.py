#!/usr/bin/env python3
"""Cross-check frozen live receipts, blinding, bucket boundaries and accounting."""
import collections
import hashlib
import json
import math
import sys
from pathlib import Path


def read(path):
    return json.loads(path.read_text())


def main():
    root = Path(sys.argv[1])
    gold = read(root / 'gold.json')
    states = read(root / 'states.json')
    manifest = read(root / 'manifest.json')
    assert len(gold) == len(states) == manifest['cohort_size'] == 172
    assert set(gold) == set(states)
    assert collections.Counter(x['verdict'] for x in gold.values()) == {'VALID': 112, 'FIXED': 21, 'OBSOLETE': 20, 'DUPLICATE': 4, 'UNCLEAR': 15}
    assert len(manifest['calibration_ids']) == 34
    assert len(manifest['holdout_ids']) == 138
    assert set(manifest['calibration_ids']).isdisjoint(manifest['holdout_ids'])
    assert set(manifest['calibration_ids'] + manifest['holdout_ids']) == set(gold)
    forbidden = {'verdict', 'area', 'status', 'close_reason', 'human_evidence', 'task_labels', 'labels', 'duplicate_of'}
    for state in states.values():
        assert set(state['task']).isdisjoint(forbidden)
        assert not any(s in state['task']['latest_pretriage_notes'] for s in ['CANCELLED:', 'triage 2026-10-02', 'Closed:'])
        assert '2026-10-02 12:5' not in state['task']['latest_pretriage_notes']
    records = 0
    for name in ['run1', 'run2']:
        out = root / name
        summary = read(out / 'summary.json')
        q = read(out / 'questions.json')
        assert q['model'] == 'jev-1.13.0'
        assert summary['run']['questions_sha256'] == hashlib.sha256((out / 'questions.json').read_bytes()).hexdigest()
        expected_calls = []
        area = collections.Counter()
        severities = []
        for group, ids in [('overall', list(gold)), ('calibration', manifest['calibration_ids']), ('holdout', manifest['holdout_ids'])]:
            observed = summary[group]
            tally = collections.Counter()
            buckets = collections.defaultdict(lambda: [0, 0])
            labelled = correct_area = 0
            for tid in ids:
                row = read(out / (tid + '.json'))
                assert row['task_id'] == tid
                assert [r['candidate_sha'] for r in row['candidate_results']] == [c['sha'] for c in states[tid]['candidate_commits']]
                classification = row['classification']
                assert 'error' not in classification
                assert classification['response']['model'] == 'jev-1.13.0'
                answers = classification['response']['answers']
                for key in ['verdict', 'area', 'severity']:
                    answer = answers[key]
                    assert 0 <= answer['confidence'] <= 1
                    assert abs(sum(answer['probabilities'].values()) - 1) <= .06  # API rounds to hundredths
                verdict = answers['verdict']
                actual = gold[tid]['verdict']
                predicted = verdict['choice']
                tally[actual, predicted] += 1
                score = verdict['confidence']
                bucket = '>=0.9' if score >= .9 else '<0.5' if score < .5 else '0.5-0.9'
                buckets[bucket][0] += 1
                buckets[bucket][1] += actual == predicted
                if gold[tid]['area']:
                    labelled += 1
                    correct_area += answers['area']['choice'] == gold[tid]['area']
                if group == 'overall':
                    records += 1
                    expected_calls.append(classification)
                    area[answers['area']['choice']] += 1
                    severities.append(answers['severity']['score'])
                    for candidate in row['candidate_results']:
                        assert 'error' not in candidate
                        assert candidate['response']['model'] == 'jev-1.13.0'
                        assert 0 <= candidate['response']['answers']['resolves']['noul'] <= 1
                        expected_calls.append(candidate)
            assert observed['n'] == len(ids)
            assert observed['correct'] == sum(n for (a, b), n in tally.items() if a == b)
            assert observed['accuracy'] == observed['correct'] / len(ids)
            for a, row in observed['confusion_matrix'].items():
                for b, count in row.items():
                    assert count == tally[a, b]
            for bucket, counts in observed['confidence_buckets'].items():
                assert [counts['n'], counts['correct']] == buckets[bucket]
            assert (labelled, correct_area) == (observed['area_labelled_n'], observed['area_correct'])
        assert len(expected_calls) == summary['calls'] == summary['successful_calls'] == 716
        assert summary['failed_calls'] == 0
        assert sum(len(r['attempts']) for r in expected_calls) == summary['attempts'] == 716
        for key in ['input_tokens', 'output_tokens']:
            assert sum(r['response']['usage'][key] for r in expected_calls) == summary['usage'][key]
        assert math.isclose(summary['estimated_usd'], summary['usage']['input_tokens'] * .042 / 1_000_000)
        assert area == summary['area_distribution']
        assert all(0 <= value <= 3 for value in severities)
        assert summary['run']['workers'] == 8
        assert summary['models'] == ['jev-1.13.0']
        print(f'{name}: PASS 172 classifications + 544 Nouls; all subsets, buckets, cost, model and area totals agree')
    assert records == 344
    # Search the exact credential bytes locally without printing them.
    key = (Path.home() / 'Petrastella/creds/jev_cassy.txt').read_bytes().strip()
    for base in [root, Path('docs/research')]:
        for path in base.rglob('*'):
            if path.is_file():
                assert key not in path.read_bytes(), f'Credential bytes found in {path.name}'
    print('PASS blinded 172-task cohort; 344 classifications; 1,088 Nouls; credential absent from evidence and deliverables')


if __name__ == '__main__':
    main()
