"""Bounded read-only history discovery; workflow results are not named-test proof."""
from collections import defaultdict
from datetime import datetime, timedelta, timezone
import json
import re
import subprocess


def command(args, cwd=None):
    result = subprocess.run(args, text=True, capture_output=True, timeout=60, cwd=cwd)
    if result.returncode:
        raise RuntimeError(result.stderr.strip()[:500])
    return json.loads(result.stdout)


def finding(pattern, test, evidence, confidence='low', **extra):
    return {'class':'flake-prone','pattern':pattern,'file':extra.pop('file','history'),'line':extra.pop('line',0),
            'test':test,'evidence':evidence,'confidence':confidence,'status':'review',
            'fix':'Inspect named native failure/pass evidence on identical source; wait on the event or repair isolation, or delete useless coverage. Never retry-wrap or quarantine.',**extra}


def analyze(runs, issues, tree_for_sha=lambda sha: None):
    rows=[]
    grouped=defaultdict(list)
    for run in runs:
        sha=run['head_sha']
        # Never equate different source commits merely because the branch is the same.
        tree=tree_for_sha(sha)
        grouped[(tree or sha,run.get('workflow_id'),run.get('event'))].append(run)
        if run.get('run_attempt',1)>1:
            rows.append(finding('historical-rerun', '<run; inspect native tests>', run['html_url'],
                                sha=sha,attempt=run['run_attempt']))
        if run.get('event')=='merge_group' and run.get('conclusion')=='failure':
            rows.append(finding('queue-ejection','<run; inspect native tests>',run['html_url'],sha=sha))
    for (identity,workflow,event),group in grouped.items():
        outcomes={run.get('conclusion') for run in group}
        if 'failure' in outcomes and 'success' in outcomes:
            rows.append(finding('same-source-workflow-flip','<workflow; named-test proof required>',
                                ' '.join(run['html_url'] for run in group),source_identity=identity,
                                uncertainty='Different jobs, selections or environment may explain aggregate status; not a proven test flake.'))
    seen=set()
    for issue in issues:
        if issue['number'] in seen: continue
        seen.add(issue['number'])
        body=issue.get('body') or ''
        metadata=re.search(r'<!--\s*cas-queue-flake:\s*(\{.*?\})\s*-->',body,re.S)
        native=None
        if metadata:
            try: native=json.loads(metadata[1]).get('test')
            except json.JSONDecodeError: pass
        rows.append(finding('historical-issue',json.dumps(native,sort_keys=True) if native else issue['title'],
                            issue['url'],'medium',issue_number=issue['number'],state=issue.get('state'),
                            labels=issue.get('labels',[]),uncertainty='Issue-reported history; inspect original named attempts before claiming same-source nondeterminism.'))
    return rows


def collect(root):
    since=(datetime.now(timezone.utc)-timedelta(days=30)).date().isoformat()
    coverage={'since':since,'days':30,'run_limit':500,'issue_limit_per_query':100,
              'read_only':True,'errors':[]}
    runs,issues=[],[]
    try:
        repo=command(['gh','repo','view','--json','nameWithOwner'],cwd=root)['nameWithOwner']
        coverage['repository']=repo
        for page in range(1,6):
            data=command(['gh','api','-X','GET',f'repos/{repo}/actions/runs',
                          '-f',f'created=>={since}','-f','per_page=100','-f',f'page={page}'])
            runs.extend(data['workflow_runs'])
            coverage['available_runs']=data['total_count']
            if len(data['workflow_runs'])<100: break
        for term in ('flaky','quarantine'):
            issues.extend(command(['gh','issue','list','--repo',repo,'--state','all','--limit','100',
                                   '--search',f'{term} updated:>={since}',
                                   '--json','number,title,body,url,state,labels,createdAt,updatedAt']))
        coverage['runs_read']=len(runs)
        coverage['run_coverage_complete']=len(runs)>=coverage['available_runs']
        coverage['issue_coverage_may_be_capped']=len(issues)>=100
    except (OSError,RuntimeError,subprocess.TimeoutExpired,KeyError) as error:
        coverage['errors'].append(str(error))
    def tree(sha):
        if not re.fullmatch(r'[0-9a-f]{40}',sha): return None
        result=subprocess.run(['git','-C',str(root),'rev-parse','--verify',sha+'^{tree}'],capture_output=True,text=True)
        return result.stdout.strip() if result.returncode==0 else None
    return {'coverage':coverage,'findings':analyze(runs,issues,tree)}
