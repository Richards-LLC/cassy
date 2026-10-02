import argparse, hashlib, json, os, subprocess, sys
from pathlib import Path

parser=argparse.ArgumentParser(description='Measure the frozen five-fixture classifier cohort via the shared Jev CLI.')
parser.add_argument('--cas-binary', type=Path, required=True)
parser.add_argument('--artifacts', type=Path, required=True)
parser.add_argument('--key-file', type=Path)
args=parser.parse_args()
repo=Path(__file__).resolve().parents[3]
artifacts=args.artifacts.resolve()
artifacts.mkdir(parents=True,exist_ok=True)
casroot=artifacts/'eval-project/.cas'
casroot.mkdir(parents=True,exist_ok=True)
(casroot/'config.toml').write_text('[jev]\nenabled = true\nmodel = "jev-1.13.0"\n')
fixtures=json.loads((repo/'cas-cli/src/jev/fixtures/failures.json').read_text())
# Measurement states contain evidence only, never expected labels/provenance diagnoses.
paths=subprocess.check_output(['git','diff','--name-only','1dd7ca8e^','1dd7ca8e','--'],text=True).splitlines()
paths=paths[:128]
states=[]
for case in fixtures:
    hints=[]
    if case['platform']=='macos':
        if 'realpath' in case['log'] and 'illegal option' in case['log']:
            hints.append('macOS BSD realpath rejects Linux -e/-m flags; historical host portability defect')
        if 'flock' in case['log']:
            hints.append('Linux shared-rustup/cache fixtures require flock; Darwin needs an explicit platform skip')
    assert len(case['log'].encode()) <= 6144
    states.append(dict(source=case['source'],platform=case['platform'],failing_block=case['log'],touched_paths=paths,known_issue_hints=hints))
inputfile=artifacts/'states.jsonl'
inputfile.write_text(''.join(json.dumps(s)+'\n' for s in states))
environment=os.environ.copy()
environment['CAS_ROOT']=str(casroot)
if args.key_file:
    environment['TYPESAFE_API_KEY']=args.key_file.read_text().strip()
binary=args.cas_binary.resolve()
version=subprocess.check_output([str(binary),'--version'],text=True).strip()
command=[str(binary),'jev','batch','--input',str(inputfile),'--questions','@'+str(repo/'cas-cli/src/jev/failure_questions.json'),'--out',str(artifacts/'answers.json'),'--advisory']
result=subprocess.run(command,cwd=artifacts/'eval-project',env=environment,capture_output=True,text=True,timeout=55)
if result.returncode:
    print('Jev batch failed; exit',result.returncode)
    sys.exit(1)
answers=json.loads((artifacts/'answers.json').read_text())
assert len(answers)==len(fixtures)
rows=[]
for case,response in zip(fixtures,answers):
    answer=response.get('answers',{}).get('failure_class',{})
    prediction=answer.get('choice')
    row=dict(id=case['id'],expected=case['expected'],predicted=prediction,confidence=answer.get('confidence'),match=prediction in case['expected'],mentions_touched_change=response.get('answers',{}).get('mentions_touched_change',{}).get('noul'))
    rows.append(row)
    print(json.dumps(row))
matched=sum(row['match'] for row in rows)
report=dict(model='jev-1.13.0',matched=matched,total=len(fixtures),questions_sha256=hashlib.sha256((repo/'cas-cli/src/jev/failure_questions.json').read_bytes()).hexdigest(),results=rows,binary=version,limitations='Small developer/operator-labelled convenience cohort. Two supervised/derived excerpts explicitly identified. Agreement is not generalized calibration. No gate outcome/merge authorized by labels.')
(artifacts/'agreement.json').write_text(json.dumps(report,indent=2)+'\n')
print(f'Agreement: {matched}/{len(fixtures)}')
sys.exit(0 if matched>=4 else 1)
