#!/usr/bin/env python3
"""Read-only, dependency-free test value and flake-risk inventory. Never a gate."""
import argparse
from collections import Counter, defaultdict
from datetime import datetime, timezone
import json
from pathlib import Path
import re
import subprocess
import sys

CLASSES = ('flake-prone', 'dead', 'duplicate', 'wording-pinned', 'style-pinned', 'slow', 'restates-implementation')
EXCLUDE = {'.git', '.cas', 'target', 'dist', 'node_modules', 'vendor', 'build', '__pycache__', '.venv'}
SUFFIXES = {'.rs', '.js', '.jsx', '.ts', '.tsx', '.mjs', '.cjs', '.py'}
FIX = {
    'fixed-sleep': 'Wait on readiness, completion, a channel or the observable event; delete a test with no unique behavior.',
    'instant-port': 'Wait for the server shutdown/ready event; do not infer it from one connection attempt.',
    'instant-teardown': 'Join/wait for the exact process, task or cleanup event before asserting its final state.',
    'tight-deadline': 'Await the event under a runner safety deadline; use a controlled clock to test a timing contract.',
    'shared-port': 'Bind port zero and retain the listener, then pass its assigned port to this test.',
    'shared-path': 'Use a per-test temporary directory and pass the owned path to the subject.',
    'shared-state': 'Give each test its own state or restore it with a full-duration isolation guard.',
    'order-dependence': 'Reset state before each test or create an independent fixture for each test.',
    'unseeded-random': 'Seed test data and record the seed; use unique temporary resources for isolation.',
    'disabled-test': 'Confirm runner reachability and the retirement evidence; fix coverage or delete a proven dead test.',
    'duplicate-body': 'Compare both assertion paths and imports; merge only after confirming identical coverage.',
    'wording-assertion': 'Assert behavior, a stable error code or caller payload instead of human wording.',
    'style-assertion': 'Keep behavioral/DOM checks; delete appearance/source pinning owned by visual QA.',
    'no-assertion': 'Trace indirect assertions or failure-by-exception; delete if no behavior is protected.',
}


RAW = re.compile(r'(?:b|c)?r(#{0,255})\"')
LIFETIME = re.compile(r"'[A-Za-z_]\w*(?!')\b")


def lexical(source, rust=False):
    """Mask comments/literals without moving offsets. Retain literal spans for data checks.

    This is lexical triage, not a language parser: JS template interpolations
    are opaque, and macros/aliases need human review.
    """
    chars = list(source)
    literals = []
    i = 0
    while i < len(source):
        start = i
        if source.startswith('//', i) or (source[i] == '#' and not source.startswith('#[', i)):
            end = source.find('\n', i)
            i = len(source) if end < 0 else end
        elif source.startswith('/*', i):
            depth = 1
            i += 2
            while i < len(source) and depth:
                if source.startswith('/*', i): depth += 1; i += 2
                elif source.startswith('*/', i): depth -= 1; i += 2
                else: i += 1
        else:
            raw = RAW.match(source, i) if rust else None
            if raw:
                body = raw.end()
                end = source.find('"' + raw[1], body)
                i = len(source) if end < 0 else end + 1 + len(raw[1])
                literals.append((start, i, source[body:end if end >= 0 else i]))
            elif not rust and source[i]=='/' and source[:i].rstrip()[-1:] in ('(', ',', '=', '[', ':', '!'):
                i+=1; bracket=False
                while i < len(source):
                    char=source[i]
                    if char=='\\': i+=2; continue
                    if char=='[': bracket=True
                    elif char==']': bracket=False
                    elif char=='/' and not bracket:
                        i+=1
                        while i<len(source) and source[i].isalpha(): i+=1
                        break
                    i+=1
                literals.append((start,i,source[start+1:i-1]))
            elif source[i:i+3] in ('"""', "'''"):
                quote = source[i:i+3]
                end = source.find(quote, i+3)
                i = len(source) if end < 0 else end+3
                literals.append((start, i, source[start+3:i-3]))
            elif source[i] in ('"', '`') or (source[i] == "'" and not (rust and LIFETIME.match(source, i))):
                quote = source[i]
                i += 1
                while i < len(source):
                    if source[i] == '\\': i += 2
                    elif source[i] == quote: i += 1; break
                    else: i += 1
                literals.append((start, i, source[start+1:i-1]))
            else:
                i += 1
                continue
        for j in range(start, i):
            if chars[j] != '\n': chars[j] = ' '
    return ''.join(chars), literals


def test_file(path):
    return bool(re.search(r'(^|/)(tests?|__tests__|e2e|specs?)/|(^|/)(test[-_])|[._-](tests?|spec|journey)[._-]', path))


def blocks(code):
    stack, close = [], {}
    for match in re.finditer(r'[{}()\[\]]', code):
        char=match[0]
        if char in '{([': stack.append((char,match.start()))
        elif stack and stack[-1][0] == {'}':'{',')':'(',']':'['}[char]:
            _,start=stack.pop(); close[start]=match.end()
    return close


def scopes(path, source, code):
    close = blocks(code)
    tests = []
    cfg = []
    if path.endswith('.rs'):
        for match in re.finditer(r'#\s*\[cfg\s*\(test\)\s*\]\s*(?:pub\s+)?mod\s+\w+\s*\{', code):
            begin = code.rfind('{', match.start(), match.end())
            cfg.append((begin, close.get(begin, len(code))))
        for match in re.finditer(r'\b(?:async\s+)?fn\s+(\w+)\s*\(', code):
            begin = code.find('{', match.end())
            if begin < 0 or begin not in close: continue
            prefix = code[max(0, match.start()-300):match.start()]
            # Only attributes immediately preceding this function count.
            prefix = re.split(r'[};]', prefix)[-1]
            direct = re.search(r'#\s*\[\s*(?:\w+::)*test(?:\([^]]*\))?\s*\]', prefix)
            if direct: tests.append((match.start(), close[begin], match[1]))
    elif path.endswith('.py'):
        for match in re.finditer(r'^\s*(?:async\s+)?def\s+(test_\w+)\s*\(', code, re.M):
            end = re.search(r'^\S|^    def ', code[match.end():], re.M)
            tests.append((match.start(), match.end()+end.start() if end else len(code), match[1]))
    else:
        for match in re.finditer(r'(?<![\w.])(?:test|it)(?:\.(?:skip|only|todo|each)(?:\([^\n]*?\))?)*\s*\(', code):
            paren=code.find('(',match.start(),match.end())
            call_end=close.get(paren,len(code))
            callback=re.search(r'=>\s*\{|function(?:\s+\w+)?\s*\([^)]*\)\s*\{',code[match.end():call_end])
            if not callback: continue
            begin=match.end()+callback.end()-1
            if begin not in close: continue
            label = re.match(r'\s*[\'"`]([^\n]*?)[\'"`]', source[match.end():])
            tests.append((match.start(), close[begin], label[1] if label else f'test@{match.start()}'))
    ranges = [(0, len(code))] if test_file(path) else [(a,b) for a,b,_ in tests] + cfg
    return tests, ranges


def scan(path, source):
    code, literals = lexical(source, path.endswith('.rs'))
    tests, ranges = scopes(path, source, code)
    rows = []
    def add(offset, pattern, category='flake-prone', confidence='medium', related=None):
        if not any(a <= offset < b for a,b in ranges): return
        owner = next((name for a,b,name in tests if a <= offset < b), '<test helper>')
        row = {'class': category, 'pattern': pattern, 'file': path, 'line': source.count('\n',0,offset)+1,
               'test': owner, 'confidence': confidence, 'fix': FIX[pattern],
               'evidence': source.splitlines()[source.count('\n',0,offset)].strip()[:300], 'status': 'review'}
        if related: row['related'] = related
        if row not in rows: rows.append(row)
    def container(offset):
        return next(((a,b) for a,b,_ in tests if a <= offset < b), (0,len(code)))
    def virtual(offset):
        a,_ = container(offset)
        return bool(re.search(r'\b(?:vi|jest)\.useFakeTimers\s*\(', code[a:offset]))
    delimiters = blocks(code)
    def polling(offset):
        for a,b in delimiters.items():
            if code[a] == '{' and a < offset < b:
                head = code[max(0,a-250):a]
                if re.search(r'while\b[^{};]*(?:ready|exists|try_wait|recv|connect|is_some|is_none)\s*\([^{}]*$', head): return True
                if re.search(r'\bloop\s*$', head) and re.search(r'\bif\b[^{}]*(?:ready|try_wait|recv|exists)[^{}]*\{[^{}]*break', code[a:b]): return True
        return False
    for match in re.finditer(r'\b(?:(?:std::)?thread::sleep|tokio::time::sleep|sleep|setTimeout|\w+\.waitForTimeout)\s*\(', code):
        if virtual(match.start()): continue
        if not polling(match.start()):
            add(match.start(), 'fixed-sleep')
            add(match.start(), 'fixed-sleep', 'slow')
    # Assertions only: connection failures in retry helpers are not instant verdicts.
    assertion = re.compile(r'\b(?:assert(?:_eq|_ne)?!|expect|assert\.(?:equal|ok|strictEqual))\s*\(')
    for match in assertion.finditer(code):
        opening=code.find('(',match.start(),match.end())
        end=delimiters.get(opening)
        if end is None: continue
        selected_end=end
        if path.endswith('.rs') or match[0].startswith('assert.'):
            # Ignore assertion diagnostics: they are explanatory prose, not pins.
            keep=2 if re.search(r'assert_(?:eq|ne)!|assert\.(?:equal|strictEqual)',match[0]) else 1
            position=opening+1; commas=0
            while position < end-1:
                if position in delimiters: position=delimiters[position]; continue
                if code[position]==',':
                    commas+=1
                    if commas==keep: selected_end=position; break
                position+=1
        else:
            _,scope_end=container(match.start())
            semicolon=code.find(';',end,scope_end)
            selected_end=semicolon+1 if semicolon>=0 else min(scope_end,end+200)
        offset, body = match.start(), code[match.start():selected_end]
        a,_ = container(offset)
        before = code[a:offset]
        if re.search(r'\b(?:connect|canConnect)\s*\(', body) and re.search(r'is_err|rejects|toBe\s*\(\s*false', body) and not polling(offset):
            add(offset, 'instant-port')
        teardown = re.search(r'\b(?:kill|terminate|abort)\s*\(|\.(?:stop|close)\s*\(', before)
        if teardown and re.search(r'\.exists\s*\(|existsSync|try_wait|is_process_(?:running|alive)|\bkill\s*\([^;]*,\s*(?:0|None)\s*\)', body):
            after = before[teardown.end():]
            if not re.search(r'\b(?:wait|join|wait_for_exit|wait_for_shutdown)\s*\(|\.close\s*\([^)]*\)\s*\.await|await\s+[\w.]*\b(?:exited|closed|finished)|while[^{}]*\b(?:kill|exists|try_wait)\s*\(', after) and not polling(offset): add(offset, 'instant-teardown')
        if not virtual(offset) and re.search(r'elapsed\s*\(\s*\)\s*<|\btimeout\s*:', body):
            millis = re.search(r'from_millis\s*\(\s*([\d_]+)|timeout\s*:\s*([\d_]+)', body)
            seconds = re.search(r'from_secs\s*\(\s*([\d_]+)', body)
            if (millis and int((millis[1] or millis[2]).replace('_','')) <= 1000) or (seconds and int(seconds[1].replace('_','')) <= 1): add(offset, 'tight-deadline')
        raw = source[match.start():selected_end]
        values = [text for start,end,text in literals if match.start() <= start < selected_end and source[max(0,start-1):start] != 'b']
        if re.search(r'(?<!\.)\b(?:css|styles?)\b|getComputedStyle', body) and (values or 'getComputedStyle' in body): add(offset, 'style-assertion', 'style-pinned')
        elif re.search(r'\b(?:stderr|stdout|message|reason)\b|toHaveText|toContainText|toThrow', body) and not re.search(r'\.(?:code|error_code|rule_id)\b', body):
            if any(re.search(r'[A-Za-z]', value) and re.search(r'\s', value) and not re.fullmatch(r'HTTP [1-5]\d\d|(?:PASS|FAIL|SKIP) [\w.-]+',value) for value in values): add(offset, 'wording-assertion', 'wording-pinned')
    for match in re.finditer(r'\b(?:bind|listen)\s*\([^;\n]*?\b([1-9][\d_]{1,4})\b', code): add(match.start(),'shared-port')
    for definition in re.finditer(r'\bconst\s+(\w+)\s*(?::[^=;]+)?=\s*([1-9][\d_]{1,4})\b', code):
        for use in re.finditer(r'\b(?:bind|listen)\s*\([^;\n]*?\b'+re.escape(definition[1])+r'\b',code): add(use.start(),'shared-port')
    for match in re.finditer(r'\b(?:recv_timeout|set_read_timeout|timeout)\s*\([^;]*?from_millis\s*\(\s*([\d_]+)',code):
        if int(match[1].replace('_','')) <= 1000 and not virtual(match.start()): add(match.start(),'tight-deadline')
    for match in re.finditer(r'\bstatic\s+mut\s+\w+',code): add(match.start(),'order-dependence')
    for match in re.finditer(r'\btemp_dir\s*\(\s*\)\s*\.join\s*\(',code):
        if any(not code[match.end():start].strip() and value for start,end,value in literals if match.end() <= start < match.end()+20): add(match.start(),'shared-path')
    for start,end,value in literals:
        if not re.match(r'/(?:var/)?tmp/[^\s{}$]+$',value): continue
        prefix=code[max(0,start-100):start]
        sink=re.search(r'\b(?:write|writeFileSync|open|mkdir)\s*\(\s*$',prefix)
        if sink: add(start,'shared-path')
    for match in re.finditer(r'\b(?:std::)?env::(?:set_var|remove_var|set_current_dir)\s*\(|\bglobalThis\.\w+\s*(?:\+\+|=)', code):
        a,b = container(match.start())
        if not re.search(r'TestEnvGuard|temp_home|monkeypatch',code[a:b]): add(match.start(),'shared-state')
    for match in re.finditer(r'\b(?:rand::(?:random|thread_rng|rng)|Math\.random|random\.(?:random|randint|choice))\s*(?:::<[^>]+>)?\s*\(', code): add(match.start(),'unseeded-random')
    for match in re.finditer(r'#\s*\[ignore(?:[^]]*)\]|\b(?:test|it|describe)\.(?:skip|todo)\b|\b(?:xit|xdescribe)\s*\(', code): add(match.start(),'disabled-test','dead','low')
    duplicates = defaultdict(list)
    for a,b,name in tests:
        body_start = code.find('{',a,b)
        body = source[body_start+1:b-1] if body_start >= 0 else source[a:b]
        normalized = re.sub(r'\s+',' ',body).strip()
        if normalized: duplicates[normalized].append((a,name))
        if not re.search(r'\b\w+\s*(?:::<[^>]+>)?\s*\(', code[body_start+1:b-1]) and not re.search(r'assert|expect|panic|throw', code[a:b]): add(a,'no-assertion','restates-implementation','low')
    for group in duplicates.values():
        for (first,original),(offset,name) in zip(group,group[1:]):
            add(offset,'duplicate-body','duplicate','medium',f'{path}:{source.count(chr(10),0,first)+1} ({original})')
            if rows and rows[-1]['pattern']=='duplicate-body': rows[-1]['evidence']='Both bodies: '+next(body for body,items in duplicates.items() if items is group)[:300]
    for match in re.finditer(r'^\s*(?:let|var)\s+(\w+)\s*=',code if not path.endswith('.rs') else '',re.M):
        offset,name = match.start(),match[1]
        if any(a <= offset < b for a,b,_ in tests): continue
        enclosing=[(a,b) for a,b in delimiters.items() if code[a]=='{' and a < offset < b]
        low,high=min(enclosing,key=lambda pair:pair[1]-pair[0]) if enclosing else (0,len(code))
        users = [test for a,b,test in tests if low <= a < b <= high and re.search(r'\b'+re.escape(name)+r'\s*(?:\+\+|=|\.push\s*\()',code[a:b])]
        if len(users)>1 and not re.search(r'beforeEach|setUp', code): add(offset,'order-dependence',related=users)
    return rows


def files(root):
    try:
        result = subprocess.run(['git','-C',str(root),'ls-files','-z'],capture_output=True,check=True)
        paths = [root / name.decode() for name in result.stdout.split(b'\0') if name]
    except (OSError,subprocess.CalledProcessError):
        import os
        paths=[]
        for directory, children, names in os.walk(root):
            children[:] = sorted(name for name in children if name not in EXCLUDE)
            paths.extend(Path(directory)/name for name in sorted(names))
    return sorted(path for path in paths if path.suffix in SUFFIXES and not EXCLUDE.intersection(path.relative_to(root).parts) and not path.is_symlink())


def report(root, history=None):
    findings, scanned, errors = [],0,[]
    for path in files(root):
        try:
            source = path.read_text()
            findings.extend(scan(path.relative_to(root).as_posix(),source))
            scanned+=1
        except (OSError,UnicodeError) as error: errors.append({'file':str(path),'error':str(error)})
    revision = subprocess.run(['git','-C',str(root),'rev-parse','HEAD'],text=True,capture_output=True).stdout.strip() or 'unversioned'
    if history: findings.extend(history['findings'])
    return {'schema':1,'root':str(root),'revision':revision,'generated_at':datetime.now(timezone.utc).isoformat(),
            'files_scanned':scanned,'counts':{key:sum(row['class']==key for row in findings) for key in CLASSES},
            'findings':findings,'history':history,'errors':errors,
            'limits':['Lexical candidates, not proven flakes/dead code or deletion permission.',
                      'Aliases, generated tests, JS template interpolation and indirect assertions require review.',
                      'Runtime cost/flake rate not measurable without CI timing/attempt evidence.']}


def markdown(data):
    out = [f'# Test audit: {Path(data["root"]).name}', '', f'Revision: `{data["revision"]}`. Phase 1 changed no audited tests.',
           f'Files read: {data["files_scanned"]}. Static findings are review candidates; no measured runtime saving is claimed.', '',
           '| Class | Findings |', '| --- | ---: |']
    out.extend(f'| {key} | {value} |' for key,value in data['counts'].items())
    out.extend(['','Baseline cost: not measurable in this static scan; use existing CI timing artifacts.','',
                '## Findings','','| Class / pattern | Test / location | Evidence | Fix | Confidence |','| --- | --- | --- | --- | --- |'])
    def cell(value): return str(value).replace('|','\\|').replace('\n',' ')[:700]
    for row in data['findings']:
        location=f'{row.get("file", "history")}:{row.get("line",0)}'
        evidence=row.get('evidence','')
        if row.get('related'): evidence += ' Related: '+str(row['related'])
        out.append('| '+' | '.join(map(cell,[row['class']+' / '+row['pattern'],row.get('test','')+' / '+location,evidence,row['fix'],row['confidence']]))+' |')
    out.extend(['','## Limits','']+data['limits'])
    if data.get('history'): out.extend(['', 'History collection: `'+json.dumps(data['history'].get('coverage',{}),sort_keys=True)+'`'])
    return '\n'.join(out)+'\n'


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root',type=Path,default=Path.cwd())
    parser.add_argument('--output',type=Path,help='artifact directory for audit.json and REPORT.md; default stdout JSON')
    parser.add_argument('--history',action='store_true',help='read about thirty days of GitHub CI/issues (gh required)')
    parser.add_argument('--history-evidence',type=Path,help='additional read-only historical findings from Cassy/CI evidence')
    args=parser.parse_args()
    history=None
    if args.history:
        from history import collect
        history=collect(args.root.resolve())
    if args.history_evidence:
        extra=json.loads(args.history_evidence.read_text())
        history=history or {'findings':[],'coverage':{}}
        history['findings'].extend(extra['findings'])
        history['coverage']['supplement']={'path':str(args.history_evidence),'coverage':extra.get('coverage',{})}
    data=report(args.root.resolve(),history)
    if args.output:
        args.output.mkdir(parents=True,exist_ok=True)
        (args.output/'audit.json').write_text(json.dumps(data,indent=2)+'\n')
        (args.output/'REPORT.md').write_text(markdown(data))
        print(json.dumps({'counts':data['counts'],'files_scanned':data['files_scanned'],'output':str(args.output),'errors':len(data['errors'])}))
    else: print(json.dumps(data))
    return 1 if data['errors'] or (history and history.get('coverage',{}).get('errors')) else 0


if __name__=='__main__': sys.exit(main())
