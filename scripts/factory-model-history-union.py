import csv,json,statistics as st,collections
MAIN='docs/factory/data/factory-model-history-2026-09-06.csv'
EXTRA=['/tmp/fmh-support/factory-model-history-2026-09-06.csv','/tmp/fmh-pippenz/factory-model-history-2026-09-06.csv']
def load(p,home):
    out=[]
    for r in csv.DictReader(open(p)):
        r['home']=home; out.append(r)
    return out
main=load(MAIN,'default'); extra=load(EXTRA[0],'support+alt')+load(EXTRA[1],'pippenz')
def d(r): return (r['session_first_at'] or r['spawn_at'] or r['lease_acquired_at'] or r['task_created_at'] or '')[:10]
def key(r): return (r['project'],r['worker_name'],r['session_id'],r['task_id'])
seen={key(r):r for r in main}
added=0
for r in extra:
    if r['transcript_joined']!='yes' and r['harness']=='db': continue   # db-only rows are identical across runs
    k=key(r)
    if k in seen and seen[k]['transcript_joined']=='yes': continue
    # a db-only row in main for the same worker/task gets superseded by the joined row from another home
    dbk=[(kk,v) for kk,v in seen.items() if kk[0]==r['project'] and kk[1]==r['worker_name'] and kk[3]==r['task_id'] and v['harness']=='db']
    for kk,v in dbk: del seen[kk]
    seen[k]=r; added+=1
rows=list(seen.values())
# a spawn/lease row that never joined a transcript is superseded when the same worker+task has a joined row in any home
joined_keys={(r['project'],r['worker_name'],r['task_id']) for r in rows if r['transcript_joined']=='yes'}
rows=[r for r in rows if r['transcript_joined']=='yes' or (r['project'],r['worker_name'],r['task_id']) not in joined_keys]
H=[r for r in rows if '2026-08-20'<=d(r)<='2026-09-06']
print('union rows',len(rows),'added from other homes',added,'horizon',len(H)); print('horizon joined',sum(1 for r in H if r['transcript_joined']=='yes'),'unjoined with model',sum(1 for r in H if r['transcript_joined']!='yes' and r['model']),'db-only',sum(1 for r in H if not r['model']))
cols=list(main[0].keys())
with open('docs/factory/data/factory-model-history-2026-09-06-allhomes-horizon.csv','w',newline='') as fh:
    w=csv.DictWriter(fh,fieldnames=cols); w.writeheader(); w.writerows(sorted(H,key=lambda r:(r['project'],d(r))))
PROJECTS=['cas-src','gabber-studio','ozer','abundant-mines','rocketship-template','Penguinz','Woodworking','pulse-card','petra-stella-cloud','mecha_cassy']
def f(x):
    try: return float(x)
    except: return None
def agg(rs):
    n=len(rs); joined=[r for r in rs if r['transcript_joined']=='yes' or (r['harness']!='db' and f(r['output_tokens']))]
    tasks={}
    for r in rs:
        if r['task_id']: tasks.setdefault((r['project'],r['task_id']),r)
    delivered=sum(1 for r in tasks.values() if r['task_closed']=='yes')
    sb=sum(int(r['send_back_count'] or 0) for r in tasks.values())
    urg=sum(int(r['urgent_stop_count'] or 0) for r in tasks.values())
    push=[f(r['minutes_to_first_push']) for r in rs if f(r['minutes_to_first_push']) is not None]
    outs=[f(r['output_tokens']) for r in joined if f(r['output_tokens'])]
    tools=[f(r['tool_calls']) for r in joined if f(r['tool_calls'])]
    _seen=set(); costs=[]
    for r in joined:
        if r['cost_usd'] and (r['project'],r['worker_name'],r['session_id']) not in _seen:
            _seen.add((r['project'],r['worker_name'],r['session_id'])); costs.append(f(r['cost_usd']))
    deliv_priced=len({(r['project'],r['task_id']) for r in joined if r['cost_usd'] and r['task_id'] and r['task_closed']=='yes'})
    return dict(sessions=n,with_tokens=len(joined),miss=(n-len(joined))/n if n else 0,tasks=len(tasks),delivered=delivered,sendbacks=sb,sb_rate=sb/delivered if delivered else None,urgent=urg,
        med_push=st.median(push) if push else None,med_out=st.median(outs) if outs else None,med_tools=st.median(tools) if tools else None,cost_total=sum(costs) if costs else None,cost_per=(sum(costs)/deliv_priced) if costs and deliv_priced else None,deliv_priced=deliv_priced)
def fmt(a):
    g=lambda v,s='%.0f': (format(v,',.0f') if s=='%,.0f' else (s%v)) if v is not None else '—'
    return f"{a['sessions']} | {a['with_tokens']} | {a['miss']:.0%} | {a['delivered']} | {a['sendbacks']} | {g(a['sb_rate'],'%.0f%%') if a['sb_rate'] is None else '%.0f%%'%(a['sb_rate']*100)} | {a['urgent']} | {g(a['med_push'],'%.1f')} | {g(a['med_out'],'%,.0f')} | {g(a['med_tools'])} | {('$%.2f'%a['cost_per']) if a['cost_per'] is not None else '—'}"
hdr='| Project | Model / effort | Sessions | With tokens | Miss rate | Tasks delivered | Send-backs | Send-back rate | Urgent stops | Median min to first push | Median output / delivered task | Median tool calls | Cost / delivered task @ list |\n|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|'
print(hdr)
per={}
for p in PROJECTS:
    P=[r for r in H if r['project']==p]
    for me in sorted(set((r['model'],r['effort']) for r in P)):
        rs=[r for r in P if (r['model'],r['effort'])==me]
        if len(rs)<3 and me[0]: continue
        a=agg(rs); per[(p,me)]=a
        print(f"| {p} | {me[0] or '(no model: DB-only rows)'} / {me[1] or '—'} | {fmt(a)}")
print()
print('| Model / effort (all projects) | '+hdr.split('| Model / effort | ')[1])
print('|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|')
overall={}
for me in sorted(set((r['model'],r['effort']) for r in H)):
    rs=[r for r in H if (r['model'],r['effort'])==me]
    if len(rs)<3 and me[0]: continue
    a=agg(rs); overall[me]=a
    print(f"| {me[0] or '(no model)'} / {me[1] or '—'} | {fmt(a)}")
json.dump({'per':{f'{k[0]}|{k[1][0]}|{k[1][1]}':v for k,v in per.items()},'overall':{f'{k[0]}|{k[1]}':v for k,v in overall.items()}},open('/tmp/union_agg.json','w'))
print('--- astra rows in horizon')
for r in H:
    if r['model']=='gpt-6-astra': print(r['project'],r['worker_name'],r['effort'],r['task_id'],r['task_closed'],'sb',r['send_back_count'],'out',r['output_tokens'],'tools',r['tool_calls'],'push',r['minutes_to_first_push'],'cost',r['cost_usd'],r['home'],r['transcript_joined'])
print('--- cas-src reconciliation (extractor union, horizon, joined rows)')
C=[r for r in H if r['project']=='cas-src']
print(collections.Counter((r['model'],r['effort'],r['transcript_joined']) for r in C))

# write the union scorecard
out=[]
def rec(scope,project,me,a):
    return dict(scope=scope,project=project,model=me[0] or '',effort=me[1] or '',sessions=a['sessions'],sessions_with_tokens=a['with_tokens'],miss_rate=round(a['miss'],4),tasks_delivered=a['delivered'],send_backs=a['sendbacks'],send_back_rate=('' if a['sb_rate'] is None else round(a['sb_rate'],4)),urgent_stops=a['urgent'],median_minutes_to_first_push=('' if a['med_push'] is None else round(a['med_push'],2)),median_output_tokens_per_delivered_task=('' if a['med_out'] is None else round(a['med_out'])),median_tool_calls_per_delivered_task=('' if a['med_tools'] is None else round(a['med_tools'])),cost_usd_total=('' if a['cost_total'] is None else round(a['cost_total'],2)),cost_usd_per_delivered_task=('' if a['cost_per'] is None else round(a['cost_per'],2)),delivered_tasks_priced=a['deliv_priced'])
for (p,me),a in per.items(): out.append(rec('project',p,me,a))
for me,a in overall.items(): out.append(rec('overall','',me,a))
with open('docs/factory/data/factory-model-scorecard-2026-09-06-allhomes-horizon.csv','w',newline='') as fh:
    w=csv.DictWriter(fh,fieldnames=list(out[0].keys())); w.writeheader(); w.writerows(out)
print('scorecard rows',len(out))
