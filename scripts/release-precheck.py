#!/usr/bin/env python3
"""Run the queue's required validation on hosted runners at an immutable SHA."""
import argparse
import datetime as dt
import fcntl
import json
from pathlib import Path
import re
import subprocess
import shutil
import sys
import time


def command(*args):
    result = subprocess.run(args, text=True, capture_output=True, timeout=45)
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or result.stdout.strip() or f'{args[0]} failed')
    return result.stdout.strip()


def save(path, receipt):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix('.tmp')
    temporary.write_text(json.dumps(receipt, indent=2)+'\n')
    temporary.replace(path)


def list_runs(repository, ref, sha):
    return json.loads(command('gh','run','list','--repo',repository,'--workflow','ci.yml',
                             '--branch',ref,'--commit',sha,'--event','workflow_dispatch',
                             '--limit','100','--json',
                             'databaseId,headSha,url,status,conclusion,createdAt'))


def discover_run(receipt):
    # Compare server identities, never the workstation's clock. The durable
    # pre-dispatch snapshot also lets a later invocation recover a delayed run.
    for attempt in range(6):
        runs = [run for run in list_runs(receipt['repository'], receipt['ref'], receipt['sha'])
                if run['headSha'] == receipt['sha']
                and run['databaseId'] not in receipt['previous_run_ids']]
        if len(runs) > 1:
            raise RuntimeError('Multiple new precheck runs found; inspect gh run list before selecting a receipt')
        if runs:
            return runs[0]
        if attempt < 5:
            time.sleep(1)
    raise RuntimeError(f"Dispatch for {receipt['ref']} is not visible yet; retry this command to recover it "
                       '(no second dispatch will be sent)')


def read_receipt(path, sha, ref):
    receipt = json.loads(path.read_text())
    if not isinstance(receipt, dict) or receipt.get('sha') != sha or receipt.get('ref') != ref:
        raise RuntimeError('Invalid precheck receipt: SHA/ref does not match the requested commit')
    if not isinstance(receipt.get('repository'), str) or not receipt['repository']:
        raise RuntimeError('Invalid precheck receipt: repository is missing')
    if not receipt.get('run_id') and not isinstance(receipt.get('previous_run_ids'), list):
        raise RuntimeError('Invalid precheck receipt: run ID or dispatch snapshot is missing')
    return receipt


def refresh(receipt):
    run = json.loads(command('gh','run','view',str(receipt['run_id']),
                            '--repo',receipt['repository'],'--json',
                            'headSha,status,conclusion,createdAt,startedAt,updatedAt,jobs,url'))
    if run['headSha'] != receipt['sha']:
        raise RuntimeError('Hosted run SHA does not match the precheck receipt')
    receipt.update(status=run['status'], conclusion=run['conclusion'], jobs=run['jobs'])
    start = dt.datetime.fromisoformat(run['createdAt'].replace('Z','+00:00'))
    end = (dt.datetime.fromisoformat(run['updatedAt'].replace('Z','+00:00'))
           if run['status']=='completed' else dt.datetime.now(dt.timezone.utc))
    receipt['elapsed_minutes'] = round((end-start).total_seconds()/60, 2)
    receipt['october_dispatch_minutes'] = [48, 53]


def cleanup(receipt):
    if receipt['status'] != 'completed' or receipt.get('ref_cleaned'):
        return
    ref = 'refs/heads/'+receipt['ref']
    current = command('git','ls-remote','origin',ref).split()
    if current:
        if current[0] != receipt['sha']:
            raise RuntimeError('Precheck ref moved; refusing to delete another commit')
        command('git','push',f"--force-with-lease={ref}:{receipt['sha']}",'origin',':'+ref)
    receipt['ref_cleaned'] = True


def execute_locked(args, sha, ref, path):
    if path.exists():
        receipt = read_receipt(path, sha, ref)
        if receipt.get('run_id'):
            refresh(receipt)
        else:
            run = discover_run(receipt)
            receipt.update(run_id=run['databaseId'], run_url=run['url'],
                           status=run['status'], conclusion=run['conclusion'])
    elif args.status:
        raise RuntimeError(f'No precheck receipt exists at {path}; dispatch the commit first')
    else:
        origin = command('git','config','--get','remote.origin.url')
        match = re.fullmatch(r'(?:https://github\.com/|git@github\.com:|ssh://git@github\.com/)([^/]+/[^/]+?)(?:\.git)?/?',origin)
        if not match:
            raise RuntimeError('Origin must name a GitHub repository; configure remote.origin.url first')
        repository = json.loads(command('gh','repo','view',match[1],
                                        '--json','nameWithOwner'))['nameWithOwner']
        previous = list_runs(repository, ref, sha)
        command('git','push','origin',f'{sha}:refs/heads/{ref}')
        receipt = dict(sha=sha, ref=ref, repository=repository, status='dispatch_unknown',
                       conclusion=None, previous_run_ids=[run['databaseId'] for run in previous],
                       dispatched_at=dt.datetime.now(dt.timezone.utc).isoformat())
        # An interrupted/timed-out gh may have dispatched successfully. Save
        # intent first and never redispatch an uncertain request automatically.
        save(path, receipt)
        output = command('gh','workflow','run','ci.yml','--repo',repository,'--ref',ref)
        url = re.search(r'https://[^\s]+/actions/runs/(\d+)',output)
        run = ({'databaseId':int(url[1]), 'url':url[0], 'status':'queued','conclusion':None}
               if url else discover_run(receipt))
        receipt.update(run_id=run['databaseId'], run_url=run['url'],
                       status=run['status'], conclusion=run['conclusion'])
    save(path, receipt)
    cleanup(receipt)
    save(path, receipt)
    if args.json:
        print(json.dumps(receipt))
    else:
        state = receipt.get('conclusion') or receipt['status']
        mark = '[OK]' if state=='success' else ('[ERROR]' if receipt.get('conclusion') else '[INFO]')
        print(f"{mark} {state}: precheck {sha[:12]}")
        print(f"  -> cas release precheck {sha[:12]} --status")
        print(f'SHA: {sha}')
        width = max(40,shutil.get_terminal_size((80,24)).columns)
        def fitted(label,value):
            available = width-len(label)
            return label+(value if len(value)<=available else value[:available-3]+'...')
        print(fitted('Run: ',receipt['run_url']))
        print(fitted('Receipt: ',str(path)))
        print('--json for complete paths, URLs and job results')
        if 'elapsed_minutes' in receipt:
            print(f"Elapsed: {receipt['elapsed_minutes']} min; October dispatches: 48 and 53 min")
    return 1 if receipt.get('conclusion') not in (None,'','success') else 0


def execute(args):
    sha = command('git','rev-parse','--verify','--end-of-options',args.sha+'^{commit}')
    ref = 'precheck/'+sha
    common = Path(command('git','rev-parse','--git-common-dir')).resolve()
    path = args.receipt or common/'cas-prechecks'/f'{sha}.json'
    locks = common/'cas-prechecks'
    locks.mkdir(parents=True, exist_ok=True)
    # Serialize dispatch/recovery for this SHA, including custom receipt paths.
    with (locks/f'{sha}.lock').open('w') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        return execute_locked(args, sha, ref, path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('sha',help='Local commit to push and validate on hosted CI')
    parser.add_argument('--status',action='store_true',help='Report the existing run without dispatching again')
    parser.add_argument('--receipt',type=Path,help='Receipt path (default: Git common directory)')
    parser.add_argument('--json',action='store_true')
    args = parser.parse_args()
    try:
        return execute(args)
    except (RuntimeError,OSError,ValueError,KeyError,TypeError,subprocess.TimeoutExpired) as error:
        message = f'Precheck failed: {error}'
        if args.json:
            print(json.dumps(dict(status='error', error=message)))
        else:
            print(message,file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
