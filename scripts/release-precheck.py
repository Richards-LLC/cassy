#!/usr/bin/env python3
"""Run the queue's required validation on hosted runners at an immutable SHA."""
import argparse
import datetime as dt
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


def execute(args):
    sha = command('git', 'rev-parse', '--verify', '--end-of-options', args.sha+'^{commit}')
    ref = 'precheck/'+sha
    common = Path(command('git', 'rev-parse', '--git-common-dir')).resolve()
    path = args.receipt or common/'cas-prechecks'/f'{sha}.json'
    if args.status:
        receipt = json.loads(path.read_text())
        if receipt['sha'] != sha:
            raise RuntimeError('Precheck receipt SHA does not match the requested commit')
        run = json.loads(command('gh','run','view',str(receipt['run_id']),
                                '--repo',receipt['repository'],'--json',
                                'headSha,status,conclusion,createdAt,startedAt,updatedAt,jobs,url'))
        if run['headSha'] != sha:
            raise RuntimeError('Hosted run SHA does not match the precheck receipt')
        receipt.update(status=run['status'], conclusion=run['conclusion'], jobs=run['jobs'])
        start = dt.datetime.fromisoformat(run['createdAt'].replace('Z','+00:00'))
        end = (dt.datetime.fromisoformat(run['updatedAt'].replace('Z','+00:00'))
               if run['status']=='completed' else dt.datetime.now(dt.timezone.utc))
        receipt['elapsed_minutes'] = round((end-start).total_seconds()/60, 2)
        receipt['october_dispatch_minutes'] = [48, 53]
    else:
        origin = command('git','config','--get','remote.origin.url')
        match = re.fullmatch(r'(?:https://github\.com/|git@github\.com:|ssh://git@github\.com/)([^/]+/[^/]+?)(?:\.git)?/?',origin)
        if not match:
            raise RuntimeError('Origin must name a GitHub repository; configure remote.origin.url first')
        repository = json.loads(command('gh','repo','view',match[1],
                                        '--json','nameWithOwner'))['nameWithOwner']
        # Push only this immutable commit. Never change HEAD, the live hub or dependencies.
        command('git','push','origin',f'{sha}:refs/heads/{ref}')
        dispatched_at = dt.datetime.now(dt.timezone.utc).isoformat()
        output = command('gh','workflow','run','ci.yml','--repo',repository,
                         '--ref',ref)
        url = re.search(r'https://[^\s]+/actions/runs/(\d+)',output)
        run = None
        if url:
            run = {'databaseId':int(url[1]), 'url':url[0], 'status':'queued','conclusion':None}
        else:
            # Older gh releases print no run URL. Read a bounded number of times,
            # rejecting an older dispatch of this SHA rather than reporting stale proof.
            for attempt in range(6):
                runs = json.loads(command('gh','run','list','--repo',repository,'--workflow','ci.yml',
                                          '--branch',ref,'--commit',sha,'--event','workflow_dispatch',
                                          '--limit','10','--json',
                                          'databaseId,headSha,url,status,conclusion,createdAt'))
                run = next((item for item in runs if item['headSha']==sha
                            and item['createdAt'] >= dispatched_at[:19]+'Z'),None)
                if run: break
                if attempt < 5: time.sleep(1)
        if run is None:
            raise RuntimeError(f'Dispatched {ref}, but the run is not visible yet; '
                               f'gh run list --repo {repository} --branch {ref}')
        receipt = dict(sha=sha, ref=ref, repository=repository, run_id=run['databaseId'],
                       run_url=run['url'], status=run['status'], conclusion=run['conclusion'],
                       dispatched_at=dispatched_at)
    path.parent.mkdir(parents=True,exist_ok=True)
    temporary = path.with_suffix('.tmp')
    temporary.write_text(json.dumps(receipt,indent=2)+'\n')
    temporary.replace(path)
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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('sha',help='Local commit to push and validate on hosted CI')
    parser.add_argument('--status',action='store_true',help='Report the existing run without dispatching again')
    parser.add_argument('--receipt',type=Path,help='Receipt path (default: Git common directory)')
    parser.add_argument('--json',action='store_true')
    try:
        return execute(parser.parse_args())
    except (RuntimeError,OSError,ValueError,subprocess.TimeoutExpired) as error:
        print(f'Precheck failed: {error}',file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
