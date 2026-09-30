#!/usr/bin/env python3
"""Boundary tests: holdout isolation, real commit receipts, honest denominators."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('reviewer_eval', Path(__file__).with_name('reviewer-eval.py'))
evaluation = importlib.util.module_from_spec(spec)
spec.loader.exec_module(evaluation)


class ReplayBoundaries(unittest.TestCase):
    def setUp(self):
        parent = os.environ.get('REVIEW_EVAL_TEST_DIR')
        if parent:
            Path(parent).mkdir(parents=True, exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(dir=parent or None)
        self.root = Path(self.temp.name)
        self.old_root = evaluation.ROOT
        self.repo = self.root/'source'
        self.repo.mkdir()
        evaluation.ROOT = self.repo
        self.git('init', '-q')
        self.git('config', 'user.name', 'Test')
        self.git('config', 'user.email', 'test@invalid.local')
        self.file = self.repo/'contract.txt'
        self.file.write_text('base\n')
        self.commit('base')
        base = self.git('rev-parse', 'HEAD')
        self.file.write_text('defective\n')
        self.commit('delivery')
        head = self.git('rev-parse', 'HEAD')
        self.file.write_text('hidden repair sentinel\n')
        self.commit('repair')
        repair = self.git('rev-parse', 'HEAD')
        self.case = {'id': 'opaque', 'base_sha': base, 'head_sha': head,
                     'expected_fix_sha': repair, 'defect': 'HIDDEN_DEFECT',
                     'expected_fix': 'HIDDEN_FIX', 'kind': 'defect',
                     'expected_axis': 'spec', 'defect_group': 'group',
                     'scope': ['contract.txt'],
                     'task_context': {'title': 'Caller contract', 'description': 'Preserve result.',
                                      'acceptance_criteria': 'The caller receives its result.'}}
        self.sources = {'spec': 'SPEC_SOURCE', 'standards': 'STANDARDS_SOURCE',
                        'baseline': 'LEGACY_BODY', 'coding_standards': 'REAL_STANDARDS',
                        'promoted_rules': []}
        self.out = self.root/'out'

    def tearDown(self):
        evaluation.ROOT = self.old_root
        self.temp.cleanup()

    def git(self, *args, cwd=None):
        return subprocess.check_output(['git', *args], cwd=cwd or self.repo, text=True).strip()

    def commit(self, subject, cwd=None):
        self.git('add', '.', cwd=cwd)
        self.git('commit', '-qm', subject, cwd=cwd)

    def report(self, axis='spec'):
        return {'axis': axis, 'status': 'rejected', 'summary': 'Observed defect',
                'criteria': [{'criterion': 'The caller receives its result.', 'status': 'rejected', 'evidence': 'contract.txt:1'}],
                'scope_creep': [], 'findings': []}

    def test_truth_and_other_axis_sources_are_not_in_prompts(self):
        for axis in evaluation.AXES:
            text = evaluation.prompt(self.case, axis, self.sources)
            for secret in ['HIDDEN_DEFECT', 'HIDDEN_FIX', self.case['expected_fix_sha']]:
                self.assertNotIn(secret, text)
        standards = evaluation.prompt(self.case, 'standards', self.sources)
        self.assertNotIn('The caller receives its result.', standards)
        self.assertNotIn('LEGACY_BODY', standards)
        self.assertNotIn('REAL_STANDARDS', evaluation.prompt(self.case, 'spec', self.sources))

    def test_checkout_has_authentic_bounds_without_later_repair(self):
        checkout = evaluation.clone(self.case, 'spec', self.out)
        self.assertEqual(self.git('rev-parse', 'HEAD', cwd=checkout), self.case['head_sha'])
        self.assertEqual(self.git('rev-parse', 'HEAD^', cwd=checkout), self.case['base_sha'])
        self.assertEqual(self.git('remote', cwd=checkout), '')
        result = subprocess.run(['git', 'cat-file', '-e', self.case['expected_fix_sha']], cwd=checkout, capture_output=True)
        self.assertNotEqual(result.returncode, 0)
        with self.assertRaises(ValueError):
            evaluation.clone(self.case, 'spec', self.out)

    def test_report_cannot_hide_an_actual_fix(self):
        checkout = evaluation.clone(self.case, 'spec', self.out)
        (checkout/'contract.txt').write_text('repaired\n')
        self.commit('review(spec): f1 repair', cwd=checkout)
        report = self.report()
        with self.assertRaisesRegex(ValueError, 'Every actual fix'):
            evaluation.validate_report(report, self.case, 'spec', checkout)
        commit = self.git('rev-parse', 'HEAD', cwd=checkout)
        report['findings'] = [{'id': 'f1', 'source': 'The caller receives its result.', 'commit': commit, 'uncertain': False}]
        self.assertEqual(evaluation.validate_report(report, self.case, 'spec', checkout), [commit])
        report['findings'][0]['uncertain'] = True
        with self.assertRaisesRegex(ValueError, 'Uncertain'):
            evaluation.validate_report(report, self.case, 'spec', checkout)

    def test_bridge_makes_real_own_axis_commit_without_staging_another_fix(self):
        checkout = evaluation.clone(self.case, 'spec', self.out)
        bridge = evaluation.CommitBridge(checkout, 'spec', self.out/'bridge-receipts')
        (checkout/'contract.txt').write_text('repaired\n')
        (checkout/'unrelated.txt').write_text('must remain unstaged\n')
        commit = bridge.commit({'finding_id': 'f1', 'files': ['contract.txt'], 'fix': 'restore caller result'})
        self.assertEqual(self.git('show', '-s', '--format=%s', commit, cwd=checkout),
                         'review(spec): f1 restore caller result')
        self.assertEqual(self.git('show', '--format=', '--name-only', commit, cwd=checkout), 'contract.txt')
        self.assertIn('unrelated.txt', self.git('status', '--porcelain', cwd=checkout))
        self.assertNotIn('.review-bridge', self.git('status', '--porcelain', cwd=checkout))

    def test_bridge_refuses_metadata_and_outside_paths(self):
        checkout = evaluation.clone(self.case, 'standards', self.out)
        bridge = evaluation.CommitBridge(checkout, 'standards', self.out/'bridge-receipts')
        for path in ['../secret', '.git/config', '.review-bridge/response.json', str(self.file)]:
            with self.assertRaises(ValueError):
                bridge.commit({'finding_id': 'f1', 'files': [path], 'fix': 'invalid request'})
        self.assertEqual(self.git('rev-parse', 'HEAD', cwd=checkout), self.case['head_sha'])

    def test_bridge_refuses_other_files_inside_the_same_checkout(self):
        checkout = evaluation.clone(self.case, 'spec', self.out)
        bridge = evaluation.CommitBridge(checkout, 'spec', self.out/'bridge-receipts', ['contract.txt'])
        (checkout/'unrelated.txt').write_text('another axis or case owns this\n')
        with self.assertRaisesRegex(ValueError, 'outside this replay axis scope'):
            bridge.commit({'finding_id': 'f1', 'files': ['unrelated.txt'], 'fix': 'forbidden change'})
        self.assertEqual(self.git('rev-parse', 'HEAD', cwd=checkout), self.case['head_sha'])

    def test_missing_runs_are_recall_misses_and_ungraded_findings_stay_pending(self):
        report = self.report()
        report['findings'] = [{'id': 'f1', 'source': 'criterion', 'evidence': 'contract.txt:1'}]
        evaluation.write(self.out/'opaque/spec/result.json', {'report': report, 'commits': [], 'validation_error': None})
        corpus = {'cases': [self.case, dict(self.case, id='missing', defect_group='second')]}
        result = evaluation.score(corpus, self.out, {})
        row = result['rows'][0]
        self.assertEqual((row['seed_hits'], row['seed_total'], row['recall']), (0, 2, 0))
        self.assertEqual(result['pending_adjudication'], ['opaque/spec/f1'])
        self.assertIsNone(row['precision'])
        self.assertFalse(result['policy_ready'])
        labels = {'findings': {'opaque/spec/f1': {'correct': True, 'seed_match': True}}}
        row = evaluation.score(corpus, self.out, labels)['rows'][0]
        self.assertEqual(row['recall'], 0.5)

    def test_invalid_report_keeps_content_and_fix_denominators_but_cannot_supply_recall(self):
        report = self.report()
        report['findings'] = [{'id': 'f1', 'source': 'criterion', 'evidence': 'contract.txt:1'}]
        evaluation.write(self.out/'opaque/spec/result.json', {'report': report,
                         'commits': ['actual-fix'], 'validation_error': 'invalid ownership'})
        labels = {'findings': {'opaque/spec/f1': {'correct': True, 'seed_match': True}}}
        result = evaluation.score({'cases': [self.case]}, self.out, labels)
        self.assertEqual(result['combined_seed_hits'], 0)
        self.assertEqual(result['content_combined_seed_hits'], 1)
        self.assertEqual(result['rows'][0]['seed_hits'], 0)
        self.assertEqual(result['rows'][0]['fix_commits'], 1)
        self.assertEqual(result['rows'][0]['true_findings'], 1)

    def test_quoted_criteria_remain_exact_without_unsupported_strict_enum(self):
        public = {'criteria': ['The message drops "parse error".']}
        value = evaluation.schema('spec', public=public)
        self.assertEqual(value['properties']['findings']['items']['properties']['source'], {'type': 'string'})

    def test_legacy_verified_wording_does_not_get_spec_only_protocol_validation(self):
        checkout = evaluation.clone(self.case, 'baseline', self.out)
        report = self.report('baseline')
        report['status'] = 'approved'
        report['criteria'][0]['status'] = 'VERIFIED'
        self.assertEqual(evaluation.validate_report(report, self.case, 'baseline', checkout), [])

    def test_cross_axis_reverts_and_test_breakage_share_one_bad_commit_denominator(self):
        evaluation.write(self.out/'opaque/spec/result.json', {'report': self.report(),
                          'commits': ['reverted', 'test-broken', 'unproved'], 'validation_error': None})
        evaluation.write(self.out/'opaque/standards/cross-check/result.json',
                         {'decisions': [{'commit': 'reverted', 'decision': 'revert'}]})
        labels = {'commits': {'reverted': {'targeted_test_status': 'pass'},
                             'test-broken': {'targeted_test_status': 'fail'}}}
        row = evaluation.score({'cases': [self.case]}, self.out, labels)['rows'][0]
        self.assertEqual((row['bad_commits'], row['fix_commits']), (2, 3))
        self.assertAlmostEqual(row['bad_commit_rate_lower_bound'], 2/3)
        self.assertEqual(row['targeted_tests_unknown'], 1)
        baseline = evaluation.score({'cases': [self.case]}, self.out, labels)['rows'][2]
        self.assertIsNone(baseline['bad_commit_rate_lower_bound'])


if __name__ == '__main__':
    unittest.main()
