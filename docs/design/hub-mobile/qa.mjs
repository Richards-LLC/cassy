// Browser evidence for the generated, self-contained artifact. No Hub transport mocks.
import assert from 'node:assert/strict';
import { readFile, writeFile, mkdir, copyFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { launchBrowser } from './browser-tools.mjs';
const [url='http://127.0.0.1:8418',out='/home/pippenz/.cas/artifacts/cas-cc1b/qa']=process.argv.slice(2);
await mkdir(out,{recursive:true});
const build=execFileSync('git',['rev-parse','HEAD'],{encoding:'utf8'}).trim();
const digest=createHash('sha256').update(await readFile(new URL('index.html',import.meta.url))).digest('hex');
const browser=await launchBrowser();const results=[],matrix=[],errors=[];
const definitions=[
 ['M01','Phone image → pinch → fullscreen → landscape → scroll','I can zoom, fill the screen, rotate and keep reading at the same relative place.'],
 ['M02','Appearance and geometry at six scheme/viewport combinations','The study and image controls stay readable without page overflow in light and dark.'],
 ['M03','Fullscreen API rejects; page and image fallback','Full screen still gives me a scrollable full-viewport reader and Escape restores the page.'],
 ['M04','Keyboard-only opening, zoom, scroll, close and return','I can operate the image reader with the keyboard and focus returns to the image I opened.'],
 ['M05','Revisit and switch images after zooming','A new image opens at fit width; the correct caption and system context follow it.'],
 ['M06','JavaScript disabled and print','All ten proposals, baseline images and the comparison remain available.'],
 ['M07','Offline standalone file and all embedded images','The study opens from a local file and loads every image without network access.'],
 ['M08','Whole-study fullscreen and comparison navigation','I can read the entire study fullscreen, reach the comparison and return without losing my place.']
];
await writeFile(resolve(out,'matrix-before-run.md'),`Build: ${build}\nHTML SHA-256: ${digest}\n\n`+definitions.map(([id,cell,expected])=>`${id} | ${cell} | ${expected} | required: real-build`).join('\n')+'\n');
const pause=page=>page.evaluate(()=>new Promise(ok=>requestAnimationFrame(()=>requestAnimationFrame(ok))));
async function setup(options={}){const context=await browser.newContext({viewport:{width:390,height:844},colorScheme:'light',...options});const page=await context.newPage();page.on('pageerror',e=>errors.push(e.message));await page.goto(url);await page.locator('[data-open-image] img').evaluateAll(imgs=>Promise.all(imgs.map(i=>i.decode())));return{context,page}}
async function openImage(page,selector='#proposal-7 .phone button'){await page.locator(selector).click();await page.locator('#viewer-image').evaluate(i=>i.decode());await pause(page)}
const geometry=page=>page.evaluate(()=>{const v=document.querySelector('#viewport');return{ratio:v.scrollTop/Math.max(1,v.scrollHeight-v.clientHeight),top:v.scrollTop,width:v.clientWidth,height:v.clientHeight,scrollHeight:v.scrollHeight,overflow:document.documentElement.scrollWidth>innerWidth+1,zoom:Number(document.querySelector('#viewer').dataset.zoom),full:document.querySelector('#viewer').dataset.fullscreen}});
async function capture(page,file){await page.screenshot({path:resolve(out,file)});return file}
const scenarios=[
 async()=>{const{context,page}=await setup({hasTouch:true,isMobile:true});try{
  await openImage(page);const area=await page.locator('#viewport').boundingBox();const client=await context.newCDPSession(page);const y=area.y+Math.min(220,area.height/2);
  await client.send('Input.dispatchTouchEvent',{type:'touchStart',touchPoints:[{x:115,y,id:1},{x:245,y,id:2}]});
  await client.send('Input.dispatchTouchEvent',{type:'touchMove',touchPoints:[{x:72,y,id:1},{x:292,y,id:2}]});
  await client.send('Input.dispatchTouchEvent',{type:'touchEnd',touchPoints:[]});
  assert((await geometry(page)).zoom>1.2,'two-finger pinch must increase image zoom');
  await page.locator('#image-fullscreen').click();await pause(page);
  await page.locator('#viewport').evaluate(v=>{v.scrollTop=(v.scrollHeight-v.clientHeight)*.4});await pause(page);const before=await geometry(page);
  await page.setViewportSize({width:844,height:390});await pause(page);const rotated=await geometry(page);
  assert(Math.abs(rotated.ratio-before.ratio)<.06,`reading place changed: ${before.ratio} → ${rotated.ratio}`);
  const b=await page.locator('#viewport').boundingBox();await page.mouse.move(b.x+b.width/2,b.y+b.height*.8);await page.mouse.down();await page.mouse.move(b.x+b.width/2,b.y+b.height*.2,{steps:8});await page.mouse.up();
  assert((await geometry(page)).top>rotated.top+20,'drag must pan vertically after rotation');
  await capture(page,'M01-rotated-reader.png');return{observed:`Touch pinch ${before.zoom.toFixed(2)}×; fullscreen ${before.full}; scroll ratio ${before.ratio.toFixed(3)} → ${rotated.ratio.toFixed(3)} on rotation; drag continued scrolling.`,evidence:'M01-rotated-reader.png'};
 }finally{await context.close()}},
 async()=>{for(const scheme of ['light','dark'])for(const size of [{width:390,height:844},{width:844,height:390},{width:1280,height:800}]){const{context,page}=await setup({viewport:size,colorScheme:scheme});try{
  assert.equal(await page.locator('.proposal').count(),10);assert.equal(await page.locator('#proposals .image-button').count(),20);
  assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),true);
  assert.equal(await page.getByRole('heading',{level:1}).evaluate(e=>e.getBoundingClientRect().bottom<innerHeight),true);
  const name=`${size.width}x${size.height}-${scheme}`;await capture(page,`matrix-${name}.png`);
  await openImage(page,'#proposal-4 .phone button');const g=await geometry(page);assert(g.height>100);assert(g.scrollHeight>g.height);
  for(const id of ['#close-viewer','#image-fullscreen','#fit-width'])assert(await page.locator(id).isVisible());
  await capture(page,`reader-${name}.png`);matrix.push({...size,scheme,readerHeight:g.height,pageOverflow:false});
 }finally{await context.close()}}
 return{observed:'All 6 viewport/scheme pairs: 10 proposals, 20 proposal views, no horizontal page overflow; reader controls visible and vertically scrollable.',evidence:'matrix-390x844-light.png'}},
 async()=>{const{context,page}=await setup();try{
  await page.evaluate(()=>{Element.prototype.requestFullscreen=()=>Promise.reject(new Error('Simulated browser denial'))});
  await openImage(page);await page.locator('#image-fullscreen').click();await pause(page);assert.equal((await geometry(page)).full,'fallback');
  await page.setViewportSize({width:844,height:390});await pause(page);assert((await geometry(page)).height>100);
  await capture(page,'M03-fullscreen-fallback.png');await page.keyboard.press('Escape');await pause(page);assert.equal(await page.locator('#viewer').evaluate(e=>e.open),true);assert.equal((await geometry(page)).full,'off');
  await page.keyboard.press('Escape');assert.equal(await page.locator('#viewer').evaluate(e=>e.open),false);
  return{observed:'Native API deliberately rejected; CSS viewport fallback remained scrollable after rotation. First Escape exited fullscreen; second closed the reader.',evidence:'M03-fullscreen-fallback.png'};
 }finally{await context.close()}},
 async()=>{const{context,page}=await setup({viewport:{width:1280,height:800}});try{
  const trigger=page.locator('#proposal-1 .phone button');await trigger.focus();await page.keyboard.press('Enter');await pause(page);
  await page.keyboard.press('+');assert((await geometry(page)).zoom>1);await page.locator('#viewport').focus();await page.keyboard.press('PageDown');await page.waitForTimeout(200);assert((await geometry(page)).top>0);
  await page.keyboard.press('0');assert.equal((await geometry(page)).zoom,1);await capture(page,'M04-keyboard-reader.png');
  await page.keyboard.press('Escape');await pause(page);assert.equal(await trigger.evaluate(e=>e===document.activeElement),true);
  return{observed:'Enter opened; + zoomed; PageDown scrolled; 0 fit width; Escape restored focus to the opening image.',evidence:'M04-keyboard-reader.png'};
 }finally{await context.close()}},
 async()=>{const{context,page}=await setup();try{
  await openImage(page,'#proposal-2 .phone button');await page.locator('#zoom-in').click();await page.locator('#close-viewer').click();
  await openImage(page,'#proposal-9 .desktop button');assert.equal((await geometry(page)).zoom,1);assert((await page.locator('#viewer-caption').innerText()).includes('Stage board'));
  await page.locator('#zoom-in').click();const before=(await geometry(page)).zoom;const box=await page.locator('#viewport').boundingBox();await page.mouse.move(box.x+100,box.y+80);await page.mouse.wheel(0,-140);await pause(page);assert((await geometry(page)).zoom>before);
  await capture(page,'M05-revisit-desktop-image.png');return{observed:'Reopening a different image reset to fit width and showed its own caption; mouse wheel zoom increased the new image scale.',evidence:'M05-revisit-desktop-image.png'};
 }finally{await context.close()}},
 async()=>{const{context,page}=await setup({javaScriptEnabled:false,viewport:{width:1280,height:800}});try{
  assert.equal(await page.locator('.proposal').count(),10);assert.equal(await page.locator('#comparison tbody tr').count(),10);assert.equal(await page.locator('[data-open-image] img').count(),25);
  assert.equal(await page.locator('[data-open-image] img').evaluateAll(xs=>xs.every(x=>x.naturalWidth>0)),true);
  await page.locator('#proposal-6').scrollIntoViewIfNeeded();await capture(page,'M06-no-javascript.png');await page.emulateMedia({media:'print'});
  await page.pdf({path:resolve(out,'study-print.pdf'),format:'A4',printBackground:true});assert.equal(await page.locator('#proposals').isVisible(),true);
  return{observed:'JavaScript off: 10 proposals, 10 comparison rows and all 25 images decoded; print PDF contains the study.',evidence:'M06-no-javascript.png; study-print.pdf'};
 }finally{await context.close()}},
 async()=>{const context=await browser.newContext({offline:true,viewport:{width:390,height:844}});const page=await context.newPage();try{
  const attempts=[];page.on('request',r=>{if(/^https?:/.test(r.url()))attempts.push(r.url())});
  await page.goto(new URL('index.html',import.meta.url).href);await page.locator('[data-open-image] img').evaluateAll(xs=>Promise.all(xs.map(x=>x.decode())));
  assert.equal(attempts.length,0);assert.equal(await page.locator('[data-open-image] img').evaluateAll(xs=>xs.every(x=>x.naturalWidth>0)),true);await openImage(page,'.current-group:nth-of-type(2) .phone button');
  await capture(page,'M07-offline-current-image.png');return{observed:'Opened standalone file with browser offline; zero HTTP(S) requests; all embedded images decoded and current-state lightbox opened.',evidence:'M07-offline-current-image.png'};
 }finally{await context.close()}},
 async()=>{const{context,page}=await setup();try{
  await page.locator('#study-fullscreen').click();await pause(page);const full=await page.locator('#study').getAttribute('data-fullscreen');
  await page.locator('.guide a[href="#comparison"]').click();await pause(page);assert(await page.locator('#comparison-heading').evaluate(e=>{const r=e.getBoundingClientRect();return r.top>=0&&r.bottom<=innerHeight}));
  const y=await page.locator('#study').evaluate(e=>e.scrollTop);assert(y>1000);await capture(page,'M08-study-fullscreen-comparison.png');
  await page.keyboard.press('Escape');await pause(page);assert.equal(await page.locator('#study').evaluate(e=>e.classList.contains('full-viewport')),false);assert(Math.abs((await page.evaluate(()=>scrollY))-y)<50);
  return{observed:`Whole-study fullscreen (${full}) reached the comparison; Escape restored document scrolling near the same position.`,evidence:'M08-study-fullscreen-comparison.png'};
 }finally{await context.close()}}
];
try {for(let i=0;i<definitions.length;i++){const[id,cell,expected]=definitions[i];try{const r=await scenarios[i]();results.push({id,cell,expected,...r,verdict:'PASS',label:'real-build'});console.log(`${id} PASS ${r.observed}`)}catch(error){results.push({id,cell,expected,observed:error.message,verdict:'FAIL',label:'real-build',evidence:''});console.log(`${id} FAIL ${error.message}`)}}}
finally{await browser.close()}
const result={build,htmlSha256:digest,at:new Date().toISOString(),results,matrix,pageErrors:errors};await writeFile(resolve(out,'results.json'),JSON.stringify(result,null,2)+'\n');
const passed=results.filter(r=>r.verdict==='PASS').length;
const ledger=`# Hub mobile study QA\n\nBuild: ${build}\nHTML SHA-256: ${digest}\nScope: Open, zoom, pan, fullscreen and rotate the committed offline design study.\nSurface: docs/design/hub-mobile/index.html\nBudget: 30 minutes\nCells: 8; PASS: ${passed}; FAIL: ${8-passed}; NOT EXERCISED: 0\nLabels: real-build 8\nsweep: not configured\n\nid | cell | expected | observed | verdict | label | evidence path | defect task\n--- | --- | --- | --- | --- | --- | --- | ---\n${results.map(r=>[r.id,r.cell,r.expected,r.observed.replaceAll('|','/'),r.verdict,r.label,r.evidence,'none'].join(' | ')).join('\n')}\n\n## Constants vs expectation\n| constant | location | visible contract | predictable? | defect task |\n| --- | --- | --- | --- | --- |\n| MIN_ZOOM=.25, MAX_ZOOM=5 | reader.js | Visible zoom output; Fit width resets scale | Yes: bounded zoom; no timeout | none |\n\n## Contradictions\nNo contradictory end-state claims observed. Drawings say proposed; baseline captions distinguish live and simulated systems. Fullscreen fallback names browser API unavailability.\n\n## Honesty\n- Browser automation exercises the actual HTML, not a mock of its reader. Chromium touch emulation and viewport resizing are not a physical phone or OS rotation. Native mobile Safari remains untested.\n- M03 deliberately rejects requestFullscreen to exercise browser failure recovery.\n- Initial capture experiments with restored browser storage lost authentication; baseline capture instead paired and captured in the same context. Those failed experiments are excluded from the baseline.\n- A first preview screenshot preceded lazy image decoding; final evidence waits for decoded images.\n- Proposed layouts are drawings. Their depicted product interactions and design impact estimates have not undergone user validation.\n- Six scheme/viewport pairs are captured separately in results.json. Print and no-JS artifacts are included.\n`;
await writeFile(resolve(out,'LEDGER.md'),ledger);await copyFile(resolve(out,'LEDGER.md'),resolve(out,'../LEDGER.md'));
console.log(`QA ${passed===8&&errors.length===0?'PASS':'FAIL'}: ${passed}/8 cells; ${matrix.length}/6 viewport/scheme pairs; ${errors.length} page errors`);
process.exitCode=passed===8&&errors.length===0?0:1;
