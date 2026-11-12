#!/usr/bin/env python3
"""Validate saved journal data before emitting static Markdown or HTML grids."""
import argparse
from datetime import date
from decimal import Decimal, InvalidOperation
from html import escape
import json
from pathlib import Path
import sys


def read(path):
    return json.loads(path.read_text(), parse_float=Decimal)


def money(value):
    amount = Decimal(str(value))
    if not amount.is_finite() or amount < 0 or amount != amount.quantize(Decimal('0.01')):
        raise ValueError('amount must be finite, nonnegative and exact to cents')
    return amount


def text(value):
    if not isinstance(value, str) or not value.strip() or '\n' in value or '\r' in value:
        raise ValueError('entry metadata and account names must be nonempty single lines')
    return value


def entries(path, verify_sources=True):
    model = read(path)
    currency = text(model['currency'])
    if len(currency) != 3 or not currency.isalpha() or not currency.isupper():
        raise ValueError('currency must be a three-letter code')
    chart = read(path.parent / model['chart'])
    numbered = chart.get('account_numbers_enabled', False) is True
    accounts = {}
    for row in chart['accounts']:
        name = text(row.get('FullyQualifiedName') or row['Name'])
        if name in accounts:
            raise ValueError('chart contains an ambiguous account name')
        number = row.get('AcctNum') if numbered else None
        if number == '':
            number = None
        if number is not None:
            text(number)
        accounts[name] = number  # Id is an internal reference, never a display number.
    result, seen, sources = [], set(), {}
    for entry in model['entries']:
        item = {key: text(entry[key]) for key in ('id', 'date', 'type', 'document', 'name', 'memo')}
        if item['id'] in seen or date.fromisoformat(item['date']).isoformat() != item['date']:
            raise ValueError('entry identity must be unique and date must be ISO calendar format')
        seen.add(item['id'])
        posted = entry.get('posted') is True
        hypothetical = entry.get('hypothetical') is True
        void = entry.get('void') is True
        if posted and (hypothetical or void):
            raise ValueError('a hypothetical or void reversal view is not a posted entry')
        item.update(view='void_reversal' if void else 'posted' if posted else 'illustration',
                    hypothetical=hypothetical, lines=[])
        line_ids, totals = set(), [Decimal(0), Decimal(0)]
        for line in entry['lines']:
            ident, account = text(line['id']), text(line['account'])
            if ident in line_ids or account not in accounts:
                raise ValueError('line identity must be unique and account must match the saved chart')
            line_ids.add(ident)
            debit, credit = money(line.get('debit', 0)), money(line.get('credit', 0))
            if bool(debit) == bool(credit):
                raise ValueError('each journal line needs exactly one positive debit or credit')
            source = line.get('source')
            if not hypothetical:
                if not source:
                    raise ValueError('each actual amount needs a saved source record')
                if verify_sources:
                    file = path.parent / source['path']
                    if file not in sources:
                        sources[file] = read(file)
                    value = sources[file]
                    pointer = source['pointer']
                    if not pointer.startswith('/'):
                        raise ValueError('source pointer must be an absolute JSON Pointer')
                    for token in pointer[1:].split('/'):
                        token = token.replace('~1', '/').replace('~0', '~')
                        value = value[int(token)] if isinstance(value, list) else value[token]
                    if money(value) != debit + credit:
                        raise ValueError('journal amount disagrees with its saved source')
            item['lines'].append(dict(id=ident, account=account, number=accounts[account],
                                      debit=f'{debit:.2f}', credit=f'{credit:.2f}', source=source))
            totals[0] += debit; totals[1] += credit
        if not item['lines'] or totals[0] != totals[1]:
            raise ValueError('journal entry does not balance')
        item['lines'].sort(key=lambda line: Decimal(line['debit']) == 0)
        item.update(debit_total=f'{totals[0]:.2f}', credit_total=f'{totals[1]:.2f}')
        result.append(item)
    if not result:
        raise ValueError('report has no journal entries')
    return model, dict(currency=currency, entries=result, changes=[])


def amounts(report):
    values = {}
    for entry in report['entries']:
        for line in entry['lines']:
            for side in ('debit', 'credit'):
                values[entry['id'], line['id'], side] = (line[side], entry['document'], line['account'])
        for side in ('debit', 'credit'):
            values[entry['id'], '$total', side] = (entry[side + '_total'], entry['document'], 'Total')
    return values


def validate(path):
    model, report = entries(path)
    previous = model.get('previous')
    if int(model.get('revision', 1)) > 1 and not previous:
        raise ValueError('a revision needs its saved previous journal model')
    if previous:
        _, old = entries(path.parent / previous, verify_sources=False)
        if old['currency'] != report['currency']:
            raise ValueError('revision comparison cannot mix currencies')
        before, after = amounts(old), amounts(report)
        for key in sorted(before.keys() | after.keys()):
            prior = before.get(key, ('0.00', '', ''))
            current = after.get(key, ('0.00', prior[1], prior[2]))
            if prior[0] != current[0]:
                report['changes'].append(dict(document=current[1], account=current[2], side=key[2],
                                               before=prior[0], after=current[0]))
    return report


def display(value):
    return f'{Decimal(value):,.2f}'


def render(report, fmt):
    if fmt == 'json':
        return json.dumps(report, indent=2)
    html = fmt == 'html'
    safe = escape if html else lambda value: escape(value).replace('|', '\\|')
    blocks = []
    for entry in report['entries']:
        heading = safe(f"{entry['date']} · {entry['type']} {entry['document']} · {entry['name']}")
        status = {'posted': 'Posted', 'illustration': 'Illustration',
                  'void_reversal': 'Illustration — reversal view of a void; not a separately posted reversing entry'}[entry['view']]
        if entry['hypothetical']:
            status += ' — hypothetical amounts'
        rows = []
        for line in entry['lines']:
            account = safe((line['number'] + ' · ' if line['number'] else '') + line['account'])
            debit, credit = (display(line[side]) if Decimal(line[side]) else '' for side in ('debit', 'credit'))
            if html:
                side = 'debit' if debit else 'credit'
                rows.append(f'<tr class="{side}"><th scope="row">{account}</th><td>{debit}</td><td>{credit}</td></tr>')
            else:
                rows.append(f'| {account if debit else "&emsp;" + account} | {debit} | {credit} |')
        debit, credit = display(entry['debit_total']), display(entry['credit_total'])
        memo = safe(entry['memo'])
        provenance = sorted({line['source']['path'] + '#' + line['source']['pointer']
                             for line in entry['lines'] if line['source']})
        source = safe('; '.join(provenance)) if provenance else 'Hypothetical illustration'
        if html:
            blocks.append(f'<section class="journal-entry"><h3>{heading}</h3><p>{status}</p>'
                          f'<table class="journal-grid"><caption>{safe(report["currency"])}</caption>'
                          '<thead><tr><th scope="col">Account</th><th scope="col">Debit</th><th scope="col">Credit</th></tr></thead>'
                          f'<tbody>{"".join(rows)}</tbody><tfoot><tr><th scope="row">Total</th><td>{debit}</td><td>{credit}</td></tr></tfoot></table>'
                          f'<p class="journal-memo">{memo}</p><p class="journal-source">{source}</p></section>')
        else:
            blocks.append(f'### {heading}\n\n{status} · {report["currency"]}\n\n| Account | Debit | Credit |\n| --- | ---: | ---: |\n' +
                          '\n'.join(rows) + f'\n| **Total** | **{debit}** | **{credit}** |\n\n{memo}\n\nSource: {source}')
    if report['changes']:
        changes = [safe(f"{c['document']} · {c['account']} · {c['side']}: {display(c['before'])} → {display(c['after'])}")
                   for c in report['changes']]
        blocks.append('<section class="journal-changes"><h3>Amount changes from the prior edition</h3><ul>' +
                      ''.join(f'<li>{line}</li>' for line in changes) + '</ul></section>' if html else
                      '### Amount changes from the prior edition\n\n' + '\n'.join('- ' + line for line in changes))
    return '\n\n'.join(blocks)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('model', type=Path)
    parser.add_argument('--format', choices=('json', 'markdown', 'html'), default='markdown')
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    output = render(validate(args.model.resolve()), args.format) + '\n'
    if args.output:
        args.output.write_text(output)
    else:
        print(output, end='')


if __name__ == '__main__':
    try:
        main()
    except (ValueError, KeyError, TypeError, IndexError, OSError, InvalidOperation) as error:
        print(f'Journal validation failed: {error}', file=sys.stderr)
        sys.exit(1)
