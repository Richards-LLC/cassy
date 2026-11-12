#!/usr/bin/env python3
"""Exercise shipped journal validation/rendering with saved ledger fixtures."""
import copy
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parent.parent / 'cas-cli/src/builtins/skills/cas-html-reports/scripts/accounting-journals.py'


class Journals(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.chart = {'account_numbers_enabled': False, 'accounts': [
            {'Id': 'qbo-internal-233', 'Name': 'Hosting Income', 'AcctNum': '4000'},
            {'Id': '2', 'Name': 'Accounts Receivable'},
            {'Id': '3', 'Name': 'Bank'}, {'Id': '4', 'Name': 'Processing Fees'}]}
        self.save('chart.json', self.chart)
        self.model = {'currency': 'USD', 'chart': 'chart.json', 'revision': 1, 'entries': [{
            'id': 'invoice-17', 'date': '2026-10-31', 'type': 'Invoice', 'document': 'INV-17',
            'name': 'Example Customer', 'posted': False, 'hypothetical': True,
            'memo': 'Illustrative hosting invoice.', 'lines': [
                {'id': 'income', 'account': 'Hosting Income', 'credit': '2250.00'},
                {'id': 'receivable', 'account': 'Accounts Receivable', 'debit': '2250.00'}]}]}

    def save(self, name, value):
        (self.root/name).write_text(json.dumps(value))

    def run_cli(self, model=None, fmt='json', output=None):
        self.save('entries.json', model or self.model)
        args = [sys.executable, str(SCRIPT), str(self.root/'entries.json'), '--format', fmt]
        if output: args += ['--output', str(output)]
        return subprocess.run(args, capture_output=True, text=True)

    def test_real_account_identity_simple_compound_and_view_labels(self):
        compound = copy.deepcopy(self.model['entries'][0])
        compound.update(id='payment-17', type='Payment', document='PAY-17', lines=[
            {'id': 'ar', 'account': 'Accounts Receivable', 'credit': '2250.00'},
            {'id': 'bank', 'account': 'Bank', 'debit': '2190.00'},
            {'id': 'fee', 'account': 'Processing Fees', 'debit': '60.00'}])
        self.model['entries'].append(compound)
        for numbered, number, void, view in [(False, '4000', False, 'illustration'),
                                             (True, '4000', False, 'illustration'),
                                             (True, '', False, 'illustration'),
                                             (False, '4000', True, 'void_reversal')]:
            with self.subTest(numbered=numbered, void=void):
                self.chart['account_numbers_enabled'] = numbered
                self.chart['accounts'][0]['AcctNum'] = number
                self.save('chart.json', self.chart)
                self.model['entries'][0]['void'] = void
                result = self.run_cli()
                self.assertEqual(result.returncode, 0, result.stderr)
                report = json.loads(result.stdout)
                simple, payment = report['entries']
                self.assertEqual(simple['view'], view)
                self.assertEqual(simple['debit_total'], simple['credit_total'])
                self.assertEqual(payment['debit_total'], '2250.00')
                self.assertEqual([line['account'] for line in payment['lines']], ['Bank', 'Processing Fees', 'Accounts Receivable'])
                income = simple['lines'][1]
                self.assertEqual(income['number'], number if numbered and number else None)
                self.assertNotIn('qbo-internal-233', result.stdout)
        html = self.run_cli(fmt='html')
        self.assertEqual(html.returncode, 0, html.stderr)
        self.assertEqual(html.stdout.count('<table'), 2)
        self.assertEqual(html.stdout.count('<tfoot>'), 2)
        self.assertIn('2,190.00', html.stdout)  # caller amount, not human-facing wording
        self.assertNotIn('4000', html.stdout)
        packaged = subprocess.run([sys.executable, str(SCRIPT),
            str(SCRIPT.parent.parent/'references/examples/accounting-journals.json'), '--format', 'json'],
            capture_output=True, text=True)
        self.assertEqual(packaged.returncode, 0, packaged.stderr)
        for entry in json.loads(packaged.stdout)['entries']:
            self.assertEqual((entry['debit_total'], entry['credit_total']), ('2250.00', '2250.00'))

    def test_invalid_model_never_replaces_an_output(self):
        invalid = []
        def change(edit):
            model = copy.deepcopy(self.model); edit(model); invalid.append(model)
        change(lambda m: m['entries'][0]['lines'][0].update(credit='2250.01'))
        change(lambda m: m['entries'][0]['lines'][0].update(account='account 233'))
        change(lambda m: m['entries'][0]['lines'][0].update(credit='Hosting Income 2250.00'))
        change(lambda m: m['entries'][0]['lines'][0].update(debit='1.00'))
        change(lambda m: m.update(revision=2))
        change(lambda m: m['entries'][0].update(posted=True))
        change(lambda m: m['entries'][0].update(hypothetical=False))
        change(lambda m: m['entries'][0]['lines'][0].update(credit='NaN'))
        output = self.root/'report.html'; output.write_bytes(b'\x00\x13\xff')
        original = output.read_bytes()
        for model in invalid:
            with self.subTest(model=model):
                result = self.run_cli(model, 'html', output)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(output.read_bytes(), original)

    def test_saved_source_amounts_and_every_revised_grid_amount_are_audited(self):
        self.save('invoice.json', {'TotalAmt': 2250})
        for line in self.model['entries'][0]['lines']:
            line['source'] = {'path': 'invoice.json', 'pointer': '/TotalAmt'}
        self.model['entries'][0].update(posted=True, hypothetical=False)
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(json.loads(result.stdout)['entries'][0]['hypothetical'])
        self.save('previous.json', self.model)
        revised = copy.deepcopy(self.model)
        revised.update(revision=2, previous='previous.json')
        self.save('invoice-v2.json', {'TotalAmt': 2500})
        for line in revised['entries'][0]['lines']:
            side = 'debit' if 'debit' in line else 'credit'
            line[side] = '2500.00'
            line['source']['path'] = 'invoice-v2.json'
        result = self.run_cli(revised)
        self.assertEqual(result.returncode, 0, result.stderr)
        changes = json.loads(result.stdout)['changes']
        self.assertEqual(len(changes), 4)  # two lines and both displayed totals
        self.assertEqual({(c['before'], c['after']) for c in changes}, {('2250.00', '2500.00')})
        self.save('invoice-v2.json', {'TotalAmt': 2499})
        self.assertNotEqual(self.run_cli(revised).returncode, 0)


if __name__ == '__main__':
    unittest.main()
