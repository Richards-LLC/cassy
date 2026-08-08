import json,os,glob,re,hashlib,collections
DIRS=[os.path.expanduser("~/.claude-alt/projects/-home-pippenz-Petrastella-cas-src"),
      os.path.expanduser("~/.claude/projects/-home-pippenz-Petrastella-cas-src")]
files=[f for d in DIRS for f in glob.glob(os.path.join(d,"*.jsonl"))]
tot={'files':0,'lines':0,'bad':0}
inturn=0; interrupts=0
user_msgs=collections.defaultdict(list)   # session -> [norm hash, text]
usage=collections.Counter()
tool_err=collections.Counter()
def norm(t):
    t=re.sub(r'\s+',' ',t).strip().lower()
    return hashlib.md5(t[:600].encode()).hexdigest()
for f in files:
    tot['files']+=1
    sess=os.path.basename(f)[:8]
    try: fh=open(f,errors='replace')
    except: continue
    for line in fh:
        tot['lines']+=1
        try: o=json.loads(line)
        except: tot['bad']+=1; continue
        m=o.get('message') or {}
        role=m.get('role') or o.get('type')
        c=m.get('content')
        txt=""
        if isinstance(c,str): txt=c
        elif isinstance(c,list):
            for b in c:
                if isinstance(b,dict):
                    if b.get('type')=='text': txt+=b.get('text','')
                    if b.get('type')=='tool_result' and b.get('is_error'): tool_err['tool_error']+=1
        if role=='user' and txt:
            if 'Request interrupted' in txt: interrupts+=1
            if len(txt)>120: user_msgs[sess].append((norm(txt),txt[:160]))
        u=m.get('usage') or {}
        for k in ('input_tokens','output_tokens','cache_read_input_tokens','cache_creation_input_tokens'):
            if isinstance(u.get(k),int): usage[k]+=u[k]
# re-ask detection: identical normalized user message repeated within a session
reask=[]; total_user=0
for sess,msgs in user_msgs.items():
    total_user+=len(msgs)
    c=collections.Counter(h for h,_ in msgs)
    texts={h:t for h,t in msgs}
    for h,n in c.items():
        if n>1: reask.append((n,sess,texts[h]))
reask.sort(reverse=True)
print("CORPUS:",tot)
print("USER_MSGS(>120ch):",total_user,"  DISTINCT_SESSIONS:",len(user_msgs))
print("INTERRUPTED_TURNS:",interrupts)
print("TOOL_ERROR_RESULTS:",tool_err['tool_error'])
dup_total=sum(n-1 for n,_,_ in reask)
print("REPEATED_USER_MSGS(extra copies):",dup_total, " rate=%.1f%%"%(100.0*dup_total/max(1,total_user)))
print("TOKENS:",dict(usage))
print("--- TOP 12 REPEATED INSTRUCTIONS ---")
for n,sess,t in reask[:12]:
    print(f"  x{n} [{sess}] {t!r}")
