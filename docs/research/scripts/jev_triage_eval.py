#!/usr/bin/env python3
"""Frozen read-only Jev eval. No SDK dependency; never logs credentials.

prepare --db DB --labels JSON --out DIR
run --out DIR --questions JSON --name run1 [--workers 8]
summary --out DIR --name run1
API errors are recorded by status/class only, never response bodies or headers.
"""
import argparse
import collections
import concurrent.futures
import datetime
import hashlib
import json
import math
import re
import sqlite3
import subprocess
import time
import urllib.error
import urllib.request
from pathlib import Path

STOP = set('the a an of to and in on for with is from cas task bug tests test fix pre existing found qa follow up should'.split())
VERDICTS = ['VALID', 'FIXED', 'OBSOLETE', 'DUPLICATE', 'UNCLEAR']


def tokens(text):
    return {s for s in re.findall(r'[a-z][a-z0-9_]{2,}', text.lower()) if s not in STOP}


def similarity(a, b):
    return len(a & b) / math.sqrt(max(1, len(a) * len(b)))


def git(*args):
    return subprocess.check_output(['git', *args], text=True, errors='replace').strip()


def write(path, value):
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + '\n')


def load(path):
    return json.loads(path.read_text())


def clean_notes(notes):
    # Cut the entire post-triage tail, including lifecycle and merged deliveries.
    chunks = re.split(r'(?=\[\d{4}-\d\d-\d\d \d\d:\d\d\])', notes)
    kept = []
    for chunk in chunks:
        match = re.match(r'\[(\d{4}-\d\d-\d\d \d\d:\d\d)\]', chunk)
        if match and match[1] >= '2026-10-02 12:50':
            continue
        if re.search(r'CANCELLED:|Closed:|CAS_SYNC_STATUS|cas-02a7 relocation|Close rejected:', chunk):
            continue
        kept.append(chunk)
    return '\n'.join(kept)[-4500:]


def prepare(args):
    out = args.out
    out.mkdir(parents=True, exist_ok=True)
    if any((out / name).exists() for name in ['states.json', 'gold.json', 'manifest.json']):
        raise SystemExit('Refusing to overwrite frozen inputs; use a new output directory')
    labels = load(args.labels)
    assert len(labels) == 172, 'Authoritative cohort must contain exactly 172 ids'
    db = sqlite3.connect(Path(args.db).resolve().as_uri() + '?mode=ro', uri=True)
    db.row_factory = sqlite3.Row
    rows = [dict(db.execute('SELECT * FROM tasks WHERE id=?', (tid,)).fetchone()) for tid in labels]
    db.close()
    ref = git('rev-parse', 'origin/main')
    files = git('ls-tree', '-r', '--name-only', ref).splitlines()
    by_basename = collections.defaultdict(list)
    for f in files:
        by_basename[Path(f).name].append(f)
    task_states = {}
    for row in rows:
        text = row['title'] + '\n' + row['description'] + '\n' + clean_notes(row['notes'])
        cited = re.findall(r'[A-Za-z0-9_./-]+\.(?:rs|py|mjs|js|ts|vue|sh|md|yml|toml)', text)
        paths = []
        for raw in cited:
            if raw in files:
                paths.append(raw)
            else:
                matches = [f for f in by_basename[Path(raw).name] if f.endswith(raw.lstrip('./'))]
                if not matches and len(by_basename[Path(raw).name]) == 1:
                    matches = by_basename[Path(raw).name]
                paths.extend(matches[:2])
        paths = list(dict.fromkeys(paths))[:6]
        task_states[row['id']] = {'task': {'id': row['id'], 'title': row['title'], 'description': row['description'][:9000], 'latest_pretriage_notes': clean_notes(row['notes'])}, 'cited_paths': paths}
    all_log = git('log', ref, '--format=%H\t%s').splitlines()
    patch_cache = {}
    for tid, state in task_states.items():
        text = json.dumps(state['task'])
        words = tokens(text)
        ids = set(re.findall(r'cas-[a-f0-9]{4,}', text))
        logs = []
        if state['cited_paths']:
            logs = git('log', ref, '-200', '--format=%H\t%s', '--', *state['cited_paths']).splitlines()
        direct = [line for line in all_log if any(i in line for i in ids)]
        pool = list(dict.fromkeys(direct + logs))
        ranked = sorted(pool, key=lambda line: (any(i in line for i in ids), similarity(words, tokens(line.split('\t', 1)[1]))), reverse=True)
        selected = ranked[:4]
        candidates = []
        for line in selected:
            sha, subject = line.split('\t', 1)
            if sha not in patch_cache:
                patch_cache[sha] = git('show', '--format=', '--no-ext-diff', '--unified=3', sha)[:10000]
            candidates.append({'sha': sha, 'subject': subject, 'patch_excerpt': patch_cache[sha]})
        state['candidate_commits'] = candidates
        near = sorted((other for other in task_states if other != tid), key=lambda other: similarity(tokens(state['task']['title']), tokens(task_states[other]['task']['title'])), reverse=True)[:3]
        state['similar_tasks'] = [{'id': other, 'title': task_states[other]['task']['title'], 'description': task_states[other]['task']['description'][:1800]} for other in near]
        snippets = []
        for path in state['cited_paths'][:3]:
            source = git('show', ref + ':' + path).splitlines()
            best = sorted(range(len(source)), key=lambda i: similarity(words, tokens(source[i])), reverse=True)[:2]
            excerpt = []
            seen = set()
            for center in sorted(best):
                for i in range(max(0, center - 6), min(len(source), center + 7)):
                    if i not in seen:
                        seen.add(i)
                        excerpt.append(f'{i + 1}: {source[i]}')
            snippets.append({'path': path, 'excerpt': '\n'.join(excerpt)[:3000]})
        state['current_main_snippets'] = snippets
    gold = {}
    aliases = {'hub-web': 'hub-web', 'commander': 'hub-web', 'cloud': 'cloud', 'ci': 'ci', 'release': 'release', 'qa': 'qa', 'factory': 'factory', 'close-gate': 'close-gate', 'hooks': 'hooks', 'violet': 'slack', 'slack': 'slack'}
    for row in rows:
        label = dict(labels[row['id']])
        label['title'] = row['title']
        label['human_evidence'] = row['close_reason'] or ''
        label['task_labels'] = json.loads(row['labels'])
        areas = {aliases[s] for s in label['task_labels'] if s in aliases}
        label['area'] = next(iter(areas)) if len(areas) == 1 else None
        label['cited_shas'] = re.findall(r'\b[0-9a-f]{7,40}\b', label['human_evidence'])
        label['candidate_contains_cited_sha'] = any(c['sha'].startswith(s) for c in task_states[row['id']]['candidate_commits'] for s in label['cited_shas'])
        gold[row['id']] = label
    calibration = sorted(labels, key=lambda tid: hashlib.sha256(tid.encode()).hexdigest())[:34]
    write(out / 'states.json', task_states)
    write(out / 'gold.json', gold)
    write(out / 'manifest.json', {'created_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(), 'origin_main': ref, 'labels_sha256': hashlib.sha256(args.labels.read_bytes()).hexdigest(), 'cohort_size': len(labels), 'calibration_ids': calibration, 'holdout_ids': sorted(set(labels) - set(calibration)), 'retrieval': 'cited path log last 200 + task ids in full main log; four ranked subject candidates; three similar tasks; three source snippets; no gold evidence in model state'})
    print('prepared', len(labels), 'tasks;', sum(len(s['candidate_commits']) for s in task_states.values()), 'candidate commits')


def request(key, state, questions):
    payload = json.dumps({'model': 'jev-1.13.0', 'state': state, 'questions': questions}).encode()
    started = time.perf_counter()
    attempts = []
    for attempt in range(4):
        tick = time.perf_counter()
        req = urllib.request.Request('https://api.typesafe.ai/v1/systemone', data=payload, headers={'Authorization': 'Bearer ' + key, 'Content-Type': 'application/json'})
        try:
            with urllib.request.urlopen(req, timeout=60) as response:
                result = json.load(response)
            attempts.append({'status': 200, 'elapsed_s': time.perf_counter() - tick})
            return {'response': result, 'elapsed_s': time.perf_counter() - started, 'attempts': attempts}
        except urllib.error.HTTPError as error:
            status = error.code
            attempts.append({'status': status, 'elapsed_s': time.perf_counter() - tick})
            if status not in (429, 500, 502, 503, 504, 529) or attempt == 3:
                return {'error': f'HTTP {status}', 'elapsed_s': time.perf_counter() - started, 'attempts': attempts}
        except (OSError, ValueError) as error:
            attempts.append({'error_class': type(error).__name__, 'elapsed_s': time.perf_counter() - tick})
            if attempt == 3:
                return {'error': type(error).__name__, 'elapsed_s': time.perf_counter() - started, 'attempts': attempts}
        time.sleep(2 ** attempt)


def run(args):
    out = args.out / args.name
    out.mkdir(exist_ok=True)
    questions = load(args.questions)['questions']
    frozen_questions = load(args.questions)
    previous = out / 'questions.json'
    if previous.exists() and load(previous) != frozen_questions:
        raise SystemExit('Refusing to mix different questions in a cached run; use a new run name')
    write(previous, frozen_questions)
    states = load(args.out / 'states.json')
    state_hash = hashlib.sha256((args.out / 'states.json').read_bytes()).hexdigest()
    run_path = out / 'run.json'
    inputs_path = out / 'inputs.json'
    input_receipt = {'states_sha256': state_hash, 'questions_sha256': hashlib.sha256(args.questions.read_bytes()).hexdigest()}
    if inputs_path.exists() and load(inputs_path) != input_receipt:
        raise SystemExit('Refusing to mix changed inputs in a partial run')
    write(inputs_path, input_receipt)
    if run_path.exists() and load(run_path).get('states_sha256', state_hash) != state_hash:
        raise SystemExit('Refusing to mix changed states in a cached run')
    cached_task_n = sum((out / (tid + '.json')).exists() for tid in states)
    if cached_task_n == len(states) and run_path.exists():
        print('complete cached run; original timings preserved')
        return
    key_path = Path.home() / 'Petrastella/creds/jev_cassy.txt'
    if key_path.stat().st_mode & 0o077:
        raise SystemExit('Credential permissions must be 0600 or stricter')
    key = key_path.read_text().strip()
    if not key or '\n' in key:
        raise SystemExit('Expected one nonempty credential line')
    started = time.perf_counter()

    def evaluate(item):
        tid, state = item
        dest = out / (tid + '.json')
        if dest.exists():
            return tid, 'cached'
        results = []
        candidate_summaries = []
        for candidate in state['candidate_commits']:
            result = request(key, {'task': state['task'], 'candidate_commit': candidate}, {'resolves': questions['candidate_resolves']})
            results.append({'candidate_sha': candidate['sha'], **result})
            score = result.get('response', {}).get('answers', {}).get('resolves', {}).get('noul')
            candidate_summaries.append({**candidate, 'resolution_noul': score})
        verdict_state = {**state, 'candidate_commits': candidate_summaries}
        main = request(key, verdict_state, {k: questions[k] for k in ['verdict', 'area', 'severity']})
        write(dest, {'task_id': tid, 'candidate_results': results, 'classification': main})
        return tid, 'ok' if 'response' in main else main.get('error')

    with concurrent.futures.ThreadPoolExecutor(max_workers=args.workers) as executor:
        for tid, status in executor.map(evaluate, states.items()):
            print(tid, status, flush=True)
    write(out / 'run.json', {'wall_s': time.perf_counter() - started, 'workers': args.workers, 'cached_task_n': cached_task_n, 'questions_sha256': hashlib.sha256(args.questions.read_bytes()).hexdigest(), 'states_sha256': state_hash})


def summary(args):
    gold = load(args.out / 'gold.json')
    manifest = load(args.out / 'manifest.json')
    rows = {}
    calls = []
    for tid in gold:
        path = args.out / args.name / (tid + '.json')
        if not path.exists():
            continue
        row = load(path)
        rows[tid] = row
        calls.extend(row['candidate_results'] + [row['classification']])
    def metrics(ids):
        matrix = {v: {p: 0 for p in VERDICTS} for v in VERDICTS}
        buckets = {b: {'n': 0, 'correct': 0} for b in ['>=0.9', '0.5-0.9', '<0.5']}
        correct = 0
        area_correct = area_n = 0
        disagreements = []
        errors = []
        for tid in ids:
            answer = rows.get(tid, {}).get('classification', {}).get('response', {}).get('answers', {})
            if 'verdict' not in answer:
                errors.append(tid)
                continue
            pred = answer['verdict']['choice']
            confidence = answer['verdict']['confidence']
            actual = gold[tid]['verdict']
            matrix[actual][pred] += 1
            ok = pred == actual
            correct += ok
            bucket = '>=0.9' if confidence >= .9 else '0.5-0.9' if confidence >= .5 else '<0.5'
            buckets[bucket]['n'] += 1
            buckets[bucket]['correct'] += ok
            if not ok:
                disagreements.append({'id': tid, 'gold': actual, 'predicted': pred, 'confidence': confidence})
            if gold[tid]['area']:
                area_n += 1
                area_correct += answer['area']['choice'] == gold[tid]['area']
        n = len(ids) - len(errors)
        return {'n': n, 'correct': correct, 'accuracy': correct / n if n else None, 'errors': errors, 'confusion_matrix': matrix, 'confidence_buckets': buckets, 'area_labelled_n': area_n, 'area_correct': area_correct, 'disagreements': disagreements}
    latencies = sorted(c['elapsed_s'] for c in calls)
    successful = [c for c in calls if 'response' in c]
    usage = {k: sum(c['response'].get('usage', {}).get(k, 0) for c in successful) for k in ['input_tokens', 'output_tokens']}
    area_distribution = collections.Counter()
    area_confidence = collections.defaultdict(list)
    for row in rows.values():
        answer = row['classification'].get('response', {}).get('answers', {}).get('area')
        if answer:
            area_distribution[answer['choice']] += 1
            area_confidence[answer['choice']].append(answer['confidence'])
    value = {'overall': metrics(list(gold)), 'calibration': metrics(manifest['calibration_ids']), 'holdout': metrics(manifest['holdout_ids']), 'calls': len(calls), 'successful_calls': len(successful), 'failed_calls': len(calls) - len(successful), 'attempts': sum(len(c['attempts']) for c in calls), 'usage': usage, 'estimated_usd': usage['input_tokens'] * .042 / 1000000, 'latency_s': {'median': latencies[len(latencies)//2], 'p95': latencies[min(len(latencies)-1, math.ceil(len(latencies)*.95)-1)], 'sum': sum(latencies)}, 'run': load(args.out / args.name / 'run.json'), 'area_distribution': dict(area_distribution), 'area_mean_confidence': {k: sum(v)/len(v) for k, v in area_confidence.items()}, 'models': sorted({c['response']['model'] for c in successful})}
    write(args.out / args.name / 'summary.json', value)
    print(json.dumps({k: value[k] for k in ['calls', 'failed_calls', 'usage', 'estimated_usd', 'latency_s']}))
    print('agreement', value['overall']['correct'], '/', value['overall']['n'])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['prepare', 'run', 'summary'])
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--db')
    parser.add_argument('--labels', type=Path)
    parser.add_argument('--questions', type=Path)
    parser.add_argument('--name', default='run1')
    parser.add_argument('--workers', type=int, default=8)
    args = parser.parse_args()
    globals()[args.action](args)


if __name__ == '__main__':
    main()
