#!/usr/bin/env python3
"""Exact-tree queue job retries and flake evidence; never train admission."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

WORKFLOW = 'full-validation.yml'


def command(args, cwd=None, output=None):
    result = subprocess.run(args, cwd=cwd, stdout=output or subprocess.PIPE,
                            stderr=subprocess.PIPE, timeout=30, text=output is None)
    if result.returncode:
        raise ValueError(f'{args[0]} failed ({result.returncode}): {result.stderr}')
    return result.stdout.strip() if output is None else None


def git(repo, *args):
    return command(['git', *args], cwd=repo)


def write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(mode='w', dir=path.parent, delete=False) as stream:
        json.dump(value, stream, indent=2)
        stream.write('\n')
        temporary = stream.name
    os.replace(temporary, path)


def identity():
    return {'schema': 1, 'head': os.environ['GITHUB_SHA'],
            'branch': os.environ['GITHUB_REF_NAME'], 'run_id': os.environ['GITHUB_RUN_ID'],
            'run_attempt': int(os.environ.get('GITHUB_RUN_ATTEMPT', '1'))}


def job_identity(args):
    # Runtime selection, matrix and toolchain inputs matter even on one tree.
    inputs=json.loads(args.inputs)
    workflow=git(args.repo,'ls-tree','-r','HEAD','--','.github','scripts/train-ci.py','scripts/ci-queue-flakes.py')
    digest=hashlib.sha256(json.dumps({'workflow':workflow,'inputs':inputs,
        'os':os.environ.get('RUNNER_OS'),'arch':os.environ.get('RUNNER_ARCH')},sort_keys=True).encode()).hexdigest()
    tree=git(args.repo,'rev-parse','HEAD^{tree}')
    key=hashlib.sha256(json.dumps([tree,args.job,digest]).encode()).hexdigest()
    return {'schema':2,'scope':'job','tree':tree,'job':args.job,'job_name':args.job_name,
        'inputs_hash':digest,'key':key,'artifact':'full-gate-job-'+key}


def job_save(args):
    expected=job_identity(args)
    previous=args.out/'receipt.json'
    reused=json.loads(previous.read_text()) if previous.is_file() else {}
    if any(reused.get(key)!=value for key,value in expected.items()): reused={}
    recovery=[]
    for path in (args.repo/'target/queue-flakes').rglob('receipt.json'):
        record=json.loads(path.read_text())
        if record.get('schema')==1 and record.get('run_id')==os.environ['GITHUB_RUN_ID']:
            recovery.append(record)
    result={**identity(),**expected,'outcome':'PASS' if args.status=='success' else 'FAIL',
        'recovery':recovery,'producer_run':reused.get('producer_run',os.environ['GITHUB_RUN_ID'])}
    if reused.get('reused_from'):
        result['reused_from']=reused['reused_from']
    write(previous,result)
    return result


def job_check(args):
    expected=job_identity(args)
    result={**expected,'reused':False,'producer_run':os.environ['GITHUB_RUN_ID']}
    (args.out/'receipt.json').unlink(missing_ok=True)
    slug=os.environ.get('GITHUB_REPOSITORY') or command([args.gh,'repo','view','--json','nameWithOwner','--jq','.nameWithOwner'],args.repo)
    def api(endpoint):
        return json.loads(command([args.gh,'api',f'repos/{slug}/{endpoint}'],args.repo))
    try:
        artifacts=api('actions/artifacts?name='+expected['artifact']+'&per_page=100')['artifacts']
        for artifact in artifacts:
            if artifact['name']!=expected['artifact'] or artifact.get('expired'):
                continue
            run_id=str(artifact['workflow_run']['id'])
            if run_id==os.environ['GITHUB_RUN_ID']:
                continue
            run=api('actions/runs/'+run_id)
            if run.get('status')!='completed' or run.get('path') not in ('.github/workflows/'+WORKFLOW,WORKFLOW):
                continue
            if run.get('event') != 'merge_group':
                continue
            with tempfile.TemporaryDirectory() as temporary:
                command([args.gh,'run','download',run_id,'--name',expected['artifact'],'--dir',temporary],args.repo)
                record=json.loads((Path(temporary)/'receipt.json').read_text())
            if any(record.get(key)!=value for key,value in expected.items()) or record.get('outcome')!='PASS':
                continue
            if record.get('run_id')!=run_id or record.get('head')!=run.get('head_sha'):
                continue
            jobs=api('actions/runs/'+run_id+'/jobs?filter=latest&per_page=100')['jobs']
            proven=[job for job in jobs if job['name'].split(' / ')[-1]==args.job_name and job.get('conclusion')=='success']
            if len(proven)!=1:
                continue
            producer=record.get('producer_run',run_id)
            if args.requires_artifact:
                outputs=api('actions/runs/'+str(producer)+'/artifacts')['artifacts']
                if not any(item['name']==args.requires_artifact and not item.get('expired') for item in outputs):
                    continue
            if any(item.get('tree')!=expected['tree'] or item.get('outcome') not in ('PASS','FLAKY') for item in record.get('recovery',[])):
                continue
            # Rebind transport identity only. Keep the observation's original
            # run so a cached flake never counts as a new independent failure.
            for index,recovery in enumerate(record.get('recovery',[])):
                rebound={**recovery,'head':os.environ['GITHUB_SHA'],'tree':expected['tree'],
                    'run_id':os.environ['GITHUB_RUN_ID'],
                    'observation_run_id':recovery.get('observation_run_id',recovery['run_id'])}
                write(args.repo/'target/queue-flakes/reused'/str(index)/'receipt.json',rebound)
            result.update(reused=True,reused_from=run_id,producer_run=record.get('producer_run',run_id))
            write(args.out/'receipt.json',{**record,**identity(),'reused_from':run_id})
            break
    except (ValueError,OSError,KeyError,subprocess.SubprocessError) as error:
        # Cache outage or malformed evidence means execution, never approval.
        print('Job receipt unavailable; executing: '+str(error),file=sys.stderr)
    return result


def job_export(args):
    """Feed this run's per-job observations into the existing flake fan-in."""
    tree=git(args.repo,'rev-parse','HEAD^{tree}')
    count=0
    for path in args.evidence.rglob('receipt.json'):
        record=json.loads(path.read_text())
        if record.get('scope')!='job' or record.get('schema')!=2:
            continue
        if record.get('run_id')!=os.environ['GITHUB_RUN_ID'] or record.get('tree')!=tree:
            raise ValueError('job receipt does not belong to the evaluated run/tree')
        for recovery in record.get('recovery',[]):
            if recovery.get('tree')!=tree or recovery.get('run_id')!=os.environ['GITHUB_RUN_ID'] or recovery.get('head')!=os.environ['GITHUB_SHA']:
                raise ValueError('recovery observation does not belong to the evaluated run/tree')
            # Two jobs may run the same script step (preflight's source-built
            # terminal test and the CI tiers); each is its own reconcile slot.
            if recovery.get('runner')=='step':
                recovery={**recovery,'shard':record['job']+'/'+recovery['shard']}
            write(args.out/str(count)/'receipt.json',recovery)
            count+=1
    return {'observations':count}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    for name in ('job-check', 'job-save'):
        job = sub.add_parser(name)
        job.add_argument('--repo', type=Path, default=Path.cwd())
        job.add_argument('--job', required=True)
        job.add_argument('--job-name', required=True)
        job.add_argument('--inputs', default='{}')
        job.add_argument('--out', type=Path, required=True)
        job.add_argument('--github-output', type=Path)
        job.add_argument('--gh', default=os.environ.get('GH_BIN', 'gh'))
        job.add_argument('--requires-artifact')
        if name == 'job-save': job.add_argument('--status', required=True)
    export = sub.add_parser('job-export')
    export.add_argument('--repo', type=Path, default=Path.cwd())
    export.add_argument('--evidence', type=Path, required=True)
    export.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    result = {'job-check': job_check, 'job-save': job_save, 'job-export': job_export}[args.command](args)
    if getattr(args, 'github_output', None):
        with args.github_output.open('a') as stream:
            for key in ('reused', 'artifact', 'producer_run'):
                if key in result: stream.write(f'{key}={str(result[key]).lower()}\n')
    print(json.dumps(result))


if __name__ == '__main__':
    try:
        sys.exit(main() or 0)
    except (ValueError, OSError, KeyError, subprocess.SubprocessError) as error:
        print(f'queue evidence: {error}', file=sys.stderr)
        sys.exit(1)
