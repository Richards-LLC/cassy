import json, sys, collections
def classify(path):
    c = collections.Counter(); chars = collections.Counter()
    for line in open(path, encoding='utf-8', errors='replace'):
        try: rec = json.loads(line)
        except Exception: continue
        t = rec.get('type'); msg = rec.get('message') or {}
        content = msg.get('content')
        if not isinstance(content, list):
            if t == 'user' and isinstance(content, str):
                kind = 'worker_or_system_injection' if content.lstrip().startswith('<') or content.startswith('[cas #') else 'operator_prompt'
                c[kind] += 1; chars[kind] += len(content)
            continue
        for b in content:
            bt = b.get('type')
            if t == 'assistant' and bt == 'text':
                c['supervisor_prose'] += 1; chars['supervisor_prose'] += len(b.get('text',''))
            elif t == 'assistant' and bt == 'tool_use':
                c['tool_call'] += 1; chars['tool_call'] += len(json.dumps(b.get('input',{})))
            elif t == 'assistant' and bt == 'thinking':
                c['thinking(hidden)'] += 1
            elif t == 'user' and bt == 'tool_result':
                cc = b.get('content'); s = cc if isinstance(cc,str) else json.dumps(cc)
                c['tool_result'] += 1; chars['tool_result'] += len(s)
            elif t == 'user' and bt == 'text':
                s = b.get('text','')
                if '<teammate-message' in s or s.startswith('[cas #') or '<task-notification' in s:
                    kind = 'worker_or_director_traffic'
                elif s.lstrip().startswith('<system-reminder') or s.lstrip().startswith('<'):
                    kind = 'system_injection'
                else: kind = 'operator_prompt'
                c[kind] += 1; chars[kind] += len(s)
    return c, chars
tot = collections.Counter(); totc = collections.Counter()
for p in sys.argv[1:]:
    c, ch = classify(p); tot += c; totc += ch
    print(p.split('/')[-1][:8], dict(c), {k: v for k,v in ch.items()})
print('POOLED blocks', dict(tot)); print('POOLED chars', dict(totc))
visible = {k:v for k,v in tot.items() if k!='thinking(hidden)'}
tv = sum(visible.values()); print('visible blocks', tv, {k: f"{100*v/tv:.1f}%" for k,v in visible.items()})
cv = {k:v for k,v in totc.items() if k!='thinking(hidden)'}; tc = sum(cv.values()); print('chars', tc, {k: f"{100*v/tc:.1f}%" for k,v in cv.items()})
