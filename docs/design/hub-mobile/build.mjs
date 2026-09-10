// Offline artifact authoring. The generated index.html has no runtime dependencies.
import { readFile, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { proposalDrawing, heroDrawing, heroPhoneDrawing, tokenCSS, themeVars } from './drawings.mjs';
const dir=fileURLToPath(new URL('.',import.meta.url));
const text=await readFile(resolve(dir,'index.md'),'utf8');
const css=await readFile(resolve(dir,'study.css'),'utf8');
const js=await readFile(resolve(dir,'reader.js'),'utf8');
const artifactDir=process.argv[2];
const e=s=>String(s).replaceAll('&','&amp;').replaceAll('<','&lt;').replaceAll('>','&gt;').replaceAll('"','&quot;');
const inline=s=>e(s).replace(/\*\*(.*?)\*\*/g,'<strong>$1</strong>').replace(/`([^`]+)`/g,'<code>$1</code>');
const paragraphs=s=>s.trim().split(/\n\s*\n/).filter(Boolean).map(p=>`<p>${inline(p.replaceAll('\n',' '))}</p>`).join('\n');
const sections=Object.fromEntries(text.split(/^## /m).slice(1).map(block=>{const i=block.indexOf('\n');return[block.slice(0,i),block.slice(i+1).trim()]}));
const proposals=Object.entries(sections).filter(([t])=>/^\d\d · /.test(t)).map(([heading,body])=>{
 const fields=Object.fromEntries(body.split('\n').map(line=>{const i=line.indexOf(': ');return[line.slice(0,i),line.slice(i+2)]}));
 return {id:Number(heading.slice(0,2)),title:heading.slice(5),...fields};
});
if(proposals.length!==10)throw new Error('Exactly ten proposal sections are required.');
let previous='';try{previous=await readFile(resolve(dir,'index.html'),'utf8')}catch{}
let manifest;
if(artifactDir)manifest=JSON.parse(await readFile(resolve(artifactDir,'manifest.json'),'utf8'));
else {const block=previous.match(/<script type="application\/json" id="capture-provenance">(.*?)<\/script>/s);if(!block)throw new Error('Initial build requires the current capture directory as an argument.');manifest=JSON.parse(block[1]);}
const uri=s=>'data:image/svg+xml;base64,'+Buffer.from(s).toString('base64');
async function screenshot(file){if(artifactDir)return 'data:image/png;base64,'+(await readFile(resolve(artifactDir,file))).toString('base64');const match=previous.match(new RegExp(`data-capture="${file}" (?:src|srcset)="([^"]+)"`));if(!match)throw new Error(`Missing embedded capture ${file}`);return match[1]}
function plate({title,alt,svg,type='desktop',caption,light,dark,width,height,capture}){
 const src=svg?uri(svg):light;
 const img=`<img ${capture?`data-capture="${capture}" `:''}src="${src}" ${dark?`data-current="true"`:''} alt="${e(alt)}" width="${width}" height="${height}" data-width="${width}" loading="eager">`;
 return `<figure class="${type}"><div class="plate-label"><span>${type==='phone'?'Phone · 390 × 844':'Desktop · 1280 × 800'}</span><span>Tap to enlarge</span></div><button class="image-button" type="button" data-open-image data-title="${e(title)}" aria-label="Open ${e(title)}">${dark?`<picture><source data-dark data-capture="${capture.replace('light','dark')}" srcset="${dark}" media="(prefers-color-scheme: dark)">${img}</picture>`:img}</button><figcaption>${inline(caption)}</figcaption></figure>`;
}
const current=[];
for(const count of [1,2]){
 const figures=[];
 for(const type of ['phone','desktop']){
  const capture=`current-${count}-${type}-light.png`;
  figures.push(plate({title:`Current · ${count===1?'one live system':'one live + one simulated system'} · ${type}`,type,width:type==='phone'?390:1280,height:type==='phone'?844:800,light:await screenshot(capture),dark:await screenshot(capture.replace('light','dark')),capture,alt:`Current Commander ${type} with ${count===1?'one actual paired Soundwave Linux Hub':'Soundwave Linux plus the clearly labeled Forge desktop simulated system'}. The fleet overview, system navigation and attention region share the viewport.`,caption:count===1?'**Live capture.** One actual paired Soundwave Linux Hub, read-only access. Current fleet overview; exact viewport, no retouching.':'**Simulated second system.** Actual Commander UI with one live Soundwave Linux Hub plus a Forge desktop fixture. No second physical Hub was connected. The added system is labeled in the screen.'}));
 }
 current.push(`<section class="current-group" aria-labelledby="current-${count}"><div class="group-heading"><h3 id="current-${count}">${count===1?'One connected system':'Two-system layout'}</h3><span class="label">${count===1?'Actual paired local Hub':'One actual + one simulated'}</span></div><div class="plates">${figures.join('\n')}</div></section>`);
}
const rows=proposals.map(p=>`<tr${p.id===1?' class="recommended"':''}><th scope="row"><a href="#proposal-${p.id}">${String(p.id).padStart(2,'0')} · ${e(p.title)}</a>${p.id===1?' · Suggested first':''}</th><td>${e(p['Best for'])}</td><td class="num">${p.Effort}</td><td class="num">${p.Impact}</td></tr>`).join('\n');
const chapters=proposals.map(p=>{
 const captions={phone:p.Mobile,desktop:p.Desktop};
 return `<article class="proposal" id="proposal-${p.id}" aria-labelledby="heading-${p.id}"><div class="proposal-heading"><span class="index-number" aria-hidden="true">${String(p.id).padStart(2,'0')}</span><div><p class="eyebrow">${e(p.Silhouette)}${p.id===1?' · Suggested first prototype':''}</p><h3 id="heading-${p.id}">${e(p.title)}</h3><p>${inline(p.Rationale)}</p><div class="estimates"><span>Effort ${p.Effort}/5</span><span>Impact ${p.Impact}/5</span><span>Design estimates</span></div></div></div><div class="plates">${['phone','desktop'].map(type=>plate({title:`${String(p.id).padStart(2,'0')} · ${p.title} · ${type}`,type,svg:proposalDrawing(p.id,type),width:type==='phone'?390:1280,height:type==='phone'?844:800,alt:`Proposed ${type} ${p.title} layout. ${captions[type]} Illustrative systems and content.`,caption:`**Proposed ${type} view.** ${captions[type]} Source: authored SVG in drawings.mjs; sample content.`})).join('')}</div><p class="tradeoff"><strong>Tradeoff.</strong> ${inline(p.Tradeoff)}</p></article>`;
}).join('\n');
const captureTimes=manifest.map(m=>m.capturedAt).sort();
const header=text.split(/^## /m)[0].trim().split(/\n\s*\n/);
const sourceProse=paragraphs(sections.Provenance);
const html=`<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover"><meta name="color-scheme" content="light dark"><title>Hub in your hand — ten mobile and desktop directions · 10 September 2026</title><style>
${tokenCSS}
:root[data-scheme="light"]{${themeVars('light')};color-scheme:light}:root[data-scheme="dark"]{${themeVars('dark')};color-scheme:dark}
${css}
@media print{:root,:root[data-scheme="dark"]{${themeVars('light')};color-scheme:light}}
</style></head><body><a class="skip" href="#current">Skip to current screens</a><div id="study"><header class="hero"><div class="wrap"><p class="eyebrow">Hub in your hand / Design study / 10 September 2026</p><h1>${inline(header[1])}</h1><figure><button class="image-button" type="button" data-open-image data-title="Give the decision a wide reading column" aria-label="Enlarge the layout comparison"><picture><source srcset="${uri(heroPhoneDrawing())}" media="(max-width:650px)"><img src="${uri(heroDrawing())}" alt="Current console regions divide the phone width; the proposed first step gives one supervisor request a full reading column. Schematic, not a product screenshot." width="720" height="244"></picture></button><figcaption>One source, one request, one place to respond. A schematic of the proposed shift; real current screens follow below.</figcaption></figure></div></header>
<main class="wrap"><div class="intro">${paragraphs(header[2])}<div class="controls"><button id="study-fullscreen" class="js-control" data-fullscreen aria-pressed="false">Full screen</button><button id="theme" class="js-control">Dark view</button><a href="#comparison">Compare the ten directions</a></div><nav class="guide" aria-label="Study sections"><a href="#current">Current screens</a><a href="#proposals">Ten proposals</a><a href="#comparison">Comparison</a><a href="#reading">Reader controls</a></nav></div>
<section class="section" id="current" aria-labelledby="current-heading"><p class="eyebrow">01 / Baseline</p><h2 id="current-heading">What we have today</h2>${paragraphs(sections['Current state'])}${current.join('\n')}<p class="label" style="margin-top:24px">Captured ${e(captureTimes[0])} to ${e(captureTimes.at(-1))}. Both color schemes are included; use the appearance control above.</p></section>
<section class="section" id="proposals" aria-labelledby="proposals-heading"><p class="eyebrow">02 / Exploration</p><h2 id="proposals-heading">Ten ways into the work</h2><p>Each pair changes the navigation model, keeps the system identifiable and gives the desktop a deliberate companion view. Zoom in to inspect the drawn controls.</p>${chapters}</section>
<section class="section" id="comparison" aria-labelledby="comparison-heading"><p class="eyebrow">03 / Decision</p><h2 id="comparison-heading">Choose the visit you want to improve</h2><div class="table-scroll" tabindex="0" role="region" aria-label="Scrollable proposal comparison"><table><caption>Design estimates · 1–5 ordinal scale · higher effort means more work; higher impact means broader phone benefit</caption><thead><tr><th scope="col">Direction</th><th scope="col">Best for</th><th scope="col">Effort</th><th scope="col">Impact</th></tr></thead><tbody>${rows}</tbody></table></div>${paragraphs(sections['Choosing a first prototype'])}</section>
<section class="section reading" id="reading" aria-labelledby="reading-heading"><h2 id="reading-heading">Read at your own scale</h2>${paragraphs(sections['Reading the study'])}</section>
<footer class="provenance"><h2>Evidence and provenance</h2>${sourceProse}<p><a href="index.md">Markdown source</a> · <a href="brief.md">Concept brief and critique</a> · <a href="qa/README.md">QA receipts</a></p></footer></main>
<dialog id="viewer" aria-labelledby="viewer-title" aria-describedby="viewer-caption"><div id="viewer-frame"><header class="viewer-header"><div class="viewer-heading"><h2 id="viewer-title">Image reader</h2><button id="close-viewer" autofocus>Close</button></div><div class="viewer-tools" role="group" aria-label="Image controls"><button class="zoom" id="zoom-out" aria-label="Zoom out">−</button><button class="zoom" id="zoom-in" aria-label="Zoom in">+</button><button id="fit-width">Fit width</button><button id="image-fullscreen" data-fullscreen aria-pressed="false">Full screen</button><output id="zoom-level" aria-live="polite">100% of width</output></div><p class="reader-help">Pinch / wheel to zoom · drag / arrow keys to scroll · Escape to exit</p></header><div id="viewport" tabindex="0" role="region" aria-label="Scrollable, zoomable image"><div id="canvas"><img id="viewer-image" alt=""></div></div><div class="viewer-footer"><p id="viewer-caption"></p><span id="fullscreen-status" role="status"></span></div></div></dialog>
</div><script type="application/json" id="capture-provenance">${JSON.stringify(manifest)}</script><script>document.documentElement.classList.add('js');\n${js}</script></body></html>`;
await writeFile(resolve(dir,'index.html'),html);
await writeFile(resolve(dir,'qa/current-state.json'),JSON.stringify(manifest,null,2)+'\n');
console.log(`Built index.html: ${(Buffer.byteLength(html)/1024).toFixed(0)} KiB; 10 proposals / 20 vector views / 8 embedded current captures.`);
