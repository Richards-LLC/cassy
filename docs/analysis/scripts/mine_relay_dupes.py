import json,os,glob,re,collections
DIRS=[os.path.expanduser("~/.claude-alt/projects/-home-pippenz-Petrastella-cas-src"),
      os.path.expanduser("~/.claude/projects/-home-pippenz-Petrastella-cas-src")]
files=[f for d in DIRS for f in glob.glob(os.path.join(d,"*.jsonl"))]
inj=collections.Counter(); first={}; last={}
kinds=collections.Counter()
for f in files:
    sess=os.path.basename(f)[:8]
    for line in open(f,errors='replace'):
        try: o=json.loads(line)
        except: continue
        m=o.get('message') or {}
        if (m.get('role') or o.get('type'))!='user': continue
        c=m.get('content'); txt=c if isinstance(c,str) else "".join(b.get('text','') for b in c if isinstance(b,dict) and b.get('type')=='text') if isinstance(c,list) else ""
        if 'teammate-message' not in txt: continue
        s=re.search(r'summary="([^"]{0,120})',txt)
        if not s: continue
        summ=re.sub(r'\s+',' ',s.group(1)).strip()
        key=(sess,summ)
        inj[key]+=1
        ts=o.get('timestamp','')
        first.setdefault(key,ts); last[key]=ts
        kinds[summ.split(':')[0].split('(')[0].strip()[:40]]+=1
tot=sum(inj.values()); dupes=sum(n-1 for n in inj.values() if n>1)
multi=[(n,k) for k,n in inj.items() if n>1]
multi.sort(reverse=True)
print("TEAMMATE-MESSAGE INJECTIONS:",tot," DISTINCT:",len(inj)," EXTRA_COPIES:",dupes," dup_rate=%.1f%%"%(100.0*dupes/max(1,tot)))
print("\n--- BY KIND (top 10) ---")
for k,n in kinds.most_common(10): print(f"  {n:5d}  {k}")
print("\n--- TOP 15 DUPLICATED INJECTIONS (n, session, summary, first->last) ---")
for n,k in multi[:15]:
    print(f"  x{n} [{k[0]}] {k[1][:95]}")
    print(f"        {first[k][:19]} -> {last[k][:19]}")
