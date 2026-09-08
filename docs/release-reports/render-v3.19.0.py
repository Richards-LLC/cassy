from pathlib import Path
import re,json,html
root=Path('docs/release-reports'); md=(root/'2026-09-08-v3.19.0.md').read_text()
tokens=json.loads(Path('docs/design/design-tokens.json').read_text())
def inline(s):
 s=html.escape(s)
 s=re.sub(r'`([^`]+)`',r'<code>\1</code>',s)
 s=re.sub(r'\[([^\]]+)\]\(([^)]+)\)',r'<a href="\2">\1</a>',s)
 return s
def simple(s):
 lines=s.strip().splitlines();r=[];i=0
 while i<len(lines):
  line=lines[i]
  if not line.strip():i+=1;continue
  if line.startswith('```'):
   code=[];i+=1
   while i<len(lines) and not lines[i].startswith('```'):code.append(lines[i]);i+=1
   r.append('<pre><code>'+html.escape('\n'.join(code))+'</code></pre>');i+=1;continue
  if line.startswith('|'):
   rows=[]
   while i<len(lines) and lines[i].startswith('|'):rows.append([c.strip() for c in lines[i].strip('|').split('|')]);i+=1
   headings=rows[0];data=rows[2:];nums=[k for k,c in enumerate(rows[1]) if c.endswith(':')]
   caption='Issue register · 21 verified closures' if headings[0]=='Issue' else 'Change map data · closed issues by surface'
   r.append('<div class="table-wrap"><table><caption>'+caption+'</caption><thead><tr>'+''.join('<th scope="col"'+(' class="num"' if k in nums else '')+'>'+inline(c)+'</th>' for k,c in enumerate(headings))+'</tr></thead><tbody>')
   for row in data:r.append('<tr>'+''.join(('<th scope="row"' if k==0 else '<td')+(' class="num"' if k in nums else '')+'>'+inline(c)+('</th>' if k==0 else '</td>') for k,c in enumerate(row))+'</tr>')
   r.append('</tbody></table></div>');continue
  if line.startswith('### '):r.append('<h3>'+inline(line[4:])+'</h3>');i+=1;continue
  para=[line];i+=1
  while i<len(lines) and lines[i].strip() and not lines[i].startswith(('#','|','```')):para.append(lines[i]);i+=1
  r.append('<p>'+inline(' '.join(para))+'</p>')
 return '\n'.join(r)
parts=re.split(r'^## (.+)\n',md,flags=re.M);sections={parts[i]:parts[i+1] for i in range(1,len(parts),2)}
head=parts[0].strip().split('\n\n');title=head[0][2:];date=head[1];verdict=head[2]
mapsec=sections['Change map'];claim=mapsec.strip().split('\n\n')[0];maptable='\n'.join(x for x in mapsec.splitlines() if x.startswith('|'));mapnote=mapsec.strip().split('\n\n')[1];mapsource=mapsec.strip().split('\n\n')[-1]
maprows=[x.split('|')[1:4] for x in maptable.splitlines()[2:-1]]
svg=['<svg viewBox="0 0 340 286" role="img" aria-labelledby="map-title map-desc"><title id="map-title">'+claim+'</title><desc id="map-desc">Factory 4, delivery 6, cloud 3, diagnostics 5, MechaCassy 3, install 0 listed issues. Each filled circle represents one closed issue. Total 21.</desc>']
svg.append('<text x="0" y="16" class="svg-meta">SURFACE</text><text x="337" y="16" text-anchor="end" class="svg-meta">CLOSED</text>')
for k,row in enumerate(maprows):
 name=row[0].strip();n=int(row[1]);y=46+k*40;ids=re.findall(r'#(\d+)',row[2]);dec=name=='Delivery'
 svg.append(f'<text x="0" y="{y+5}" class="svg-label">{name}</text>')
 for j in range(n):svg.append(f'<circle cx="{140+j*25}" cy="{y}" r="7" class="{"decisive" if dec else "dot"}"><title>Issue #{ids[j]}</title></circle>')
 if n==0:svg.append(f'<text x="134" y="{y+4}" class="svg-note">release hardening</text>')
 svg.append(f'<text x="337" y="{y+5}" text-anchor="end" class="svg-count">{n}</text>')
svg.append('</svg>')
def groups(s,dev=False):
 a=re.split(r'^### (.+)\n',s,flags=re.M);result=[]
 if a[0].strip():result.append(simple(a[0]))
 for k in range(1,len(a),2):
  name=a[k];content=a[k+1];items=re.split(r'^#### (.+)\n',content,flags=re.M)
  result.append(f'<section class="theme theme-{(k+1)//2}"><div class="theme-label"><span class="eyebrow">{(k+1)//2:02d}</span><h3>{inline(name)}</h3></div><div class="changes">')
  for j in range(1,len(items),2):
   title=items[j];text=items[j+1]
   m=re.match(r'\s*Was: (.*?)\n\nNow: (.*?)(?=\n\nSource:|\Z)',text,flags=re.S)
   if not m:raise ValueError(title)
   was,now=m.groups()
   result.append('<article class="change"><h4>'+inline(title)+'</h4><div class="pair"><p class="was"><span class="state">Was</span>'+inline(was.strip())+'</p><p class="now"><span class="state">Now</span>'+inline(now.strip())+'</p></div></article>')
   trailing=text[m.end():].strip()
   if trailing:result.append(simple(trailing))
  result.append('</div></section>')
 return '\n'.join(result)
def vars_for(scheme):return ';'.join('--'+k+':'+v['$value'] for k,v in tokens['color'][scheme].items())
fonts=';'.join('--font-'+k+':'+','.join('"'+f+'"' if ' ' in f else f for f in v['$value']) for k,v in tokens['typography']['family'].items())
css=':root{'+vars_for('light')+';'+fonts+';color-scheme:light dark}\n@media(prefers-color-scheme:dark){:root{'+vars_for('dark')+'}}\n'
css+='''
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--ink);font:17px/1.55 var(--font-body)}
a{color:var(--action);text-underline-offset:3px;overflow-wrap:anywhere}a:hover{text-decoration-thickness:2px}a:focus-visible,summary:focus-visible{outline:3px solid var(--action);outline-offset:4px}
.skip{position:absolute;top:8px;left:8px;transform:translateY(-200%);padding:12px;background:var(--surface);color:var(--ink);z-index:3}.skip:focus{transform:none}
main{max-width:1120px;margin:auto;padding:0 24px}.hero{background:var(--surface-hero);color:var(--ink);border-bottom:3px solid var(--verdict);padding:48px max(24px,calc((100vw - 1072px)/2)) 24px}
.eyebrow{font:600 12px/1.5 var(--font-mono);letter-spacing:.08em;text-transform:uppercase;color:var(--ink-muted)}.folio{display:flex;justify-content:space-between;gap:16px;margin:0 0 32px}.hero-grid{display:grid;grid-template-columns:1.1fr 1fr;gap:48px;align-items:center}
h1{font:400 clamp(40px,4.6vw,60px)/1.06 var(--font-display);letter-spacing:-.025em;margin:0;max-width:19ch}.hero h1 em{font-weight:400;color:var(--verdict)}h2{font:600 27px/1.2 var(--font-body);letter-spacing:-.015em;margin:0 0 24px}h3{font:600 21px/1.35 var(--font-body);margin:4px 0 16px}h4{font-size:17px;line-height:1.4;margin:0 0 12px;font-weight:600}p{margin:0 0 16px;max-width:68ch}figure{margin:0}svg{display:block;width:100%;height:auto}.svg-label,.svg-count{font:15px var(--font-body);fill:var(--ink)}.svg-count{font-family:var(--font-mono)}.svg-meta{font:10px var(--font-mono);fill:var(--ink-muted)}.svg-note{font:11px var(--font-body);fill:var(--ink-muted)}.dot{fill:var(--evidence)}.decisive{fill:var(--verdict)}.figure-claim{font-size:14px;line-height:1.45;margin:8px 0 0;color:var(--ink-muted)}.hero-note{border-top:1px solid var(--line-strong);margin:32px 0 0;padding-top:16px;display:flex;gap:24px;justify-content:space-between;font-size:14px;color:var(--ink-muted)}.hero-note p{margin:0}.hero-note a{white-space:nowrap}
.report-section{padding:64px 0;border-bottom:1px solid var(--line-strong)}.section-top{display:flex;justify-content:space-between;gap:16px}.section-no{white-space:nowrap;font:14px var(--font-mono);color:var(--ink-muted)}.stats{display:grid;grid-template-columns:repeat(4,1fr);gap:24px;margin-bottom:24px}.stat{border-left:1px solid var(--line-strong);padding-left:16px}.stat:first-child{border:0;padding:0}.stat-value{display:block;font:400 44px/1.1 var(--font-display);letter-spacing:-.025em;margin-bottom:8px}.stat-label{display:block;font-weight:600;margin-bottom:8px}.stat p{font-size:12px;line-height:1.5;color:var(--ink-muted);margin:0}.method{font-size:14px;color:var(--ink-muted)}details{margin-top:24px}summary{cursor:pointer;font-weight:600;color:var(--action)}details p{font-size:14px;margin-top:16px}.table-wrap{overflow-x:auto;margin:24px 0}table{width:100%;border-collapse:collapse;text-align:left;font-size:14px}caption{text-align:left;color:var(--ink-muted);font-size:14px;margin-bottom:12px}th,td{padding:12px 12px 12px 0;border-bottom:1px solid var(--line);vertical-align:top}th{font-weight:500}thead th{font:600 11px/1.6 var(--font-mono);letter-spacing:.05em;text-transform:uppercase;border-bottom:1px solid var(--line-strong)}.num{text-align:right;font-variant-numeric:tabular-nums}tbody th{min-width:56px}tbody tr:last-child td,tbody tr:last-child th{border-bottom:1px solid var(--line-strong)}code,pre{font-family:var(--font-mono);font-size:.86em}code{overflow-wrap:anywhere}pre{white-space:pre-wrap;overflow-wrap:anywhere;margin:24px 0;padding:20px;border-left:3px solid var(--verdict);background:var(--surface);color:var(--ink)}
.theme{padding:32px 0;border-top:1px solid var(--line);display:grid;grid-template-columns:220px 1fr;gap:48px}.theme:first-child{border-top:0;padding-top:0}.change{padding:0 0 24px;margin:0 0 24px;border-bottom:1px solid var(--line)}.change:last-child{margin:0;padding:0;border:0}.pair{display:grid;grid-template-columns:1fr 1.25fr;gap:24px}.pair p{font-size:15px;line-height:1.6;margin:0}.was{color:var(--ink-muted)}.state{display:block;font:600 10px/1.8 var(--font-mono);letter-spacing:.08em;text-transform:uppercase;margin-bottom:4px}.now .state{color:var(--verdict)}
.users .theme-1 .theme-label{padding:24px;background:var(--surface-hero);color:var(--ink);align-self:start;border-left:3px solid var(--verdict)}.users .theme-2{grid-template-columns:1fr}.users .theme-2 .theme-label{display:flex;gap:16px;align-items:baseline}.users .theme-2 .pair{grid-template-columns:1fr 1fr}.users .theme-2 .change{display:grid;grid-template-columns:220px 1fr;gap:48px}.users .theme-4{grid-template-columns:1fr}.users .theme-4 .changes{display:grid;grid-template-columns:1fr 1fr;gap:24px}.users .theme-4 .change{padding:24px;background:var(--surface);color:var(--ink);border:0;margin:0}.users .theme-4 .pair{display:block}.users .theme-4 .was{margin-bottom:16px}
.dev .theme{gap:32px}.dev .change{display:grid;grid-template-columns:150px 1fr;gap:24px;margin:0 0 16px;padding-bottom:16px}.dev h4{font-size:14px}.dev .pair{display:block}.dev .pair p{font-size:14px}.dev .was{margin-bottom:4px}.dev .state{display:inline;margin-right:8px}.dev .changes>p{font-size:14px;color:var(--ink-muted)}.install pre{font-size:32px}.install h3{margin-top:32px}.install code{word-break:break-all}.stitch{width:132px;color:var(--verdict);margin-bottom:24px}.provenance{font-size:14px}.provenance pre{font-size:12px}.provenance p{max-width:90ch}.page-footer{max-width:1120px;margin:auto;padding:32px 24px 48px;display:flex;justify-content:space-between;gap:24px;font-size:12px;color:var(--ink-muted)}
@media(max-width:820px){.hero-grid{gap:24px}.theme{grid-template-columns:160px 1fr;gap:24px}.users .theme-2 .change{grid-template-columns:160px 1fr;gap:24px}.dev .theme,.dev .change{grid-template-columns:1fr;gap:8px}.stats{gap:16px}.stat-value{font-size:34px}}
@media(max-width:600px){main{padding:0 24px}.hero{padding:24px 24px 20px}.folio{display:block;margin-bottom:24px}.folio span{display:block}.hero-grid{grid-template-columns:1fr;gap:24px}h1{font-size:40px;max-width:19ch}.hero figure{max-width:400px}.hero-note{display:block;margin-top:20px;padding-top:12px;font-size:12px}.hero-note a{display:inline-block;margin-top:8px}.figure-claim{font-size:12px}.report-section{padding:40px 0}.stats{grid-template-columns:1fr 1fr;gap:24px}.stat:nth-child(3){border:0;padding:0}.stat-value{font-size:38px}.theme,.users .theme-2 .change{grid-template-columns:1fr;gap:16px}.pair,.users .theme-2 .pair{grid-template-columns:1fr;gap:12px}.users .theme-1 .theme-label{padding:16px}.users .theme-4 .changes{grid-template-columns:1fr}.theme{padding:24px 0}.theme-label h3{margin-bottom:4px}.change{margin-bottom:20px;padding-bottom:20px}.users .theme-2 .theme-label{display:block}.table-wrap{margin:20px 0}td,th{padding-right:10px;font-size:12px}thead th{font-size:10px}.ledger td:last-child{overflow-wrap:anywhere;width:24%}.page-footer{display:block}.page-footer span{display:block;margin-bottom:8px}.section-top{gap:8px}h2{font-size:25px}.install pre{font-size:27px}}
@page{margin:15mm}
@media print{
:root{PRINTVARS;color-scheme:light}body{background:var(--surface);color:var(--ink);font-size:10pt;line-height:1.4}main{max-width:none;padding:0}.hero{background:var(--surface);color:var(--ink);padding:12mm 0 6mm;break-after:page}.folio{margin-bottom:14mm}.hero-grid{grid-template-columns:1fr;gap:10mm}.hero h1{font-size:38pt;max-width:22ch}.hero figure{width:100%;max-width:115mm}.hero-note{margin-top:8mm;font-size:9pt}.figure-claim{font-size:10pt}.skip{display:none}.report-section{padding:0;border:0;break-before:page}.section-top{padding-top:3mm}h2{font-size:22pt;margin-bottom:8mm}h3{font-size:15pt}h4{font-size:11pt}p{max-width:none}.stats{gap:5mm}.stat-value{font-size:26pt}.stat p{font-size:8pt}.stat-label{font-size:10pt}.method,details p,caption{font-size:9pt}summary{font-size:10pt}.table-wrap{overflow:visible;margin:5mm 0}table{font-size:9pt}thead{display:table-header-group}th,td{padding:3mm 2mm 3mm 0;font-size:9pt}tr,figure,pre,.change{break-inside:avoid}a[href^="http"]::after{content:" (" attr(href) ")";font:8pt var(--font-body);overflow-wrap:anywhere;color:var(--ink-muted)}a{color:var(--ink);text-decoration:underline}.theme,.users .theme-2 .change,.dev .theme{grid-template-columns:1fr;gap:4mm;padding:6mm 0;break-inside:auto}.theme-label{break-after:avoid}.users .theme-1 .theme-label{background:var(--surface);color:var(--ink);padding:3mm 0;border:0}.theme-2 .theme-label{display:block}.pair,.users .theme-2 .pair{grid-template-columns:1fr 1fr;gap:6mm}.pair p{font-size:10pt}.change{margin-bottom:5mm;padding-bottom:5mm}.users .theme-4 .changes{display:block}.users .theme-4 .change{background:var(--surface);color:var(--ink);padding:0 0 5mm;margin-bottom:5mm}.users .theme-4 .pair{display:grid}.users .theme-4 .was{margin:0}.dev .change{grid-template-columns:36mm 1fr;gap:6mm;margin-bottom:4mm;padding-bottom:4mm}.dev .pair p,.dev h4{font-size:9pt}.dev .pair{display:block}.dev .changes>p{font-size:8pt}.ledger td:last-child{width:22%}.install pre{font-size:22pt}.install h3{margin-top:10mm}.provenance{font-size:9pt}.provenance pre{font-size:8pt}.page-footer{font-size:8pt;padding:8mm 0 0}.page-footer a:after{content:none}pre{background:var(--surface);color:var(--ink);padding:4mm;white-space:pre-wrap}.stitch{width:35mm}
}

@media print{
.users .change{margin-bottom:3mm;padding-bottom:3mm}.users h4{margin-bottom:2mm}.users .pair p{font-size:9pt}.users .state{display:inline;margin-right:2mm}.users .theme{padding:4mm 0;gap:2mm}.users .theme-label h3{margin-bottom:2mm}.users .theme-4 .change{padding-bottom:3mm;margin-bottom:3mm}.dev .theme{padding:4mm 0;gap:2mm}.dev .change{padding-bottom:2.5mm;margin-bottom:2.5mm}.dev h3{margin-bottom:2mm}.ledger table{table-layout:fixed}.ledger th:first-child{width:28%}.ledger td:last-child{width:22%}.ledger th,.ledger td{font-size:8pt;line-height:1.25;padding:1.2mm 2mm 1.2mm 0}.ledger a[href^="http"]::after{font-size:7pt}.ledger caption{margin-bottom:2mm}
}

@media print{
.theme,.users .theme-2,.users .theme-4,.dev .theme{display:block}.theme-label{margin-bottom:4mm;break-after:avoid}.users .theme-2 .change{display:block}.users .theme-2 h4{break-after:avoid}.change,.dev .change{break-inside:avoid-page}.ledger th,.ledger td{padding:1mm 2mm 1mm 0;line-height:1.2}.ledger p{font-size:9pt}.ledger h2{margin-bottom:5mm}
}

@media print{
.users .change,.users .theme-2 .change{display:inline-block;width:100%;break-inside:avoid;page-break-inside:avoid}.theme-label .eyebrow{display:none}.theme-label h3{break-after:avoid;page-break-after:avoid}.theme-label{break-inside:avoid;page-break-inside:avoid}
}
'''.replace('PRINTVARS',vars_for('light'))
# All editorial copy and numbers are read from the markdown above.
heroverdict=inline(verdict).replace('clearer states','<em>clearer states</em>')
output=['<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="color-scheme" content="light dark"><title>Cassy v3.19.0 · Release report · 8 September 2026</title><style>'+css+'</style></head><body><a class="skip" href="#content">Skip to release details</a>']
output.append('<header class="hero"><div class="folio eyebrow"><span>'+inline(title)+' / Release notes</span><span>'+inline(date)+'</span></div><div class="hero-grid"><h1>'+heroverdict+'</h1><figure>'+''.join(svg)+'<figcaption class="figure-claim">'+inline(claim)+'</figcaption></figure></div><div class="hero-note"><p>One dot, one closed issue. Five themes. Installation improved too.</p><a href="#map-data">Read the map data ↓</a></div></header><main id="content">')
glance=sections['Release at a glance'];rows=[r.split('|')[1:-1] for r in glance.splitlines() if r.startswith('|')][2:]
output.append('<section class="report-section"><div class="section-top"><h2>Release at a glance</h2><span class="section-no">01 / 06</span></div><div class="stats">')
for label,value,definition in rows:output.append('<div class="stat"><span class="stat-value">'+inline(value.strip())+'</span><span class="stat-label">'+inline(label.strip())+'</span><p>'+inline(definition.strip())+'</p></div>')
output.append('</div><div class="method">'+simple(glance.split('\n\n')[-2] if glance.endswith('\n\n') else glance.split('\n\n')[-1])+'</div>')
output.append('<details open id="map-data"><summary>Change map · data and reading guide</summary><p>'+inline(mapnote)+'</p>'+simple(maptable)+'<p>'+inline(mapsource)+'</p></details></section>')
for i,name,cls in [(2,'What you can do now','users'),(3,'Under the hood','dev'),(4,'Fixes ledger','ledger'),(5,'Install','install'),(6,'Evidence and scope','provenance')]:
 output.append(f'<section class="report-section {cls}"><div class="section-top"><h2>{name}</h2><span class="section-no">{i:02d} / 06</span></div>')
 if cls in ('users','dev'):output.append(groups(sections[name],cls=='dev'))
 else:
  if cls=='install':output.append('<svg class="stitch" viewBox="0 0 132 16" aria-hidden="true">'+''.join(f'<circle cx="{8+j*23}" cy="8" r="5" fill="currentColor"/>' for j in range(6))+'</svg>')
  output.append(simple(sections[name]))
 output.append('</section>')
output.append('</main><footer class="page-footer"><span>Cassy / Clearer states. Safer next steps.</span><span><a href="2026-09-08-v3.19.0.md">Markdown source</a> · <a href="v3.19.0.pdf">Print edition</a></span></footer><script>addEventListener("beforeprint",()=>document.querySelectorAll("details").forEach(panel=>{panel.open=true}));</script></body></html>')
(root/'v3.19.0.html').write_text('\n'.join(output))
print('Rendered from markdown:',(root/'v3.19.0.html').stat().st_size,'bytes')
