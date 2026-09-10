// Run with a fresh cas hub pair --origin <DIST_ORIGIN> --json receipt.
// The invitation is a capability: keep it outside git, revoke the device afterward.
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { launchBrowser } from './browser-tools.mjs';
const [statePath, outputDir, origin='http://127.0.0.1:8417'] = process.argv.slice(2);
if (!statePath || !outputDir) throw new Error('Usage: node capture-current.mjs PRIVATE_INVITATION_JSON ARTIFACT_DIR [DIST_ORIGIN]');
await mkdir(outputDir,{recursive:true});
const browser=await launchBrowser();
const invitation=JSON.parse(await readFile(statePath,'utf8'));
const context=await browser.newContext({viewport:{width:1280,height:800}});
const page=await context.newPage();
const receipts=[];
try {
  await page.goto(invitation.url).catch(()=>{throw new Error('Pairing page failed; invitation omitted')});
  await page.locator('input[name=url]').fill('http://127.0.0.1:4173');
  await page.locator('input[name=label]').fill('Soundwave Linux');
  await page.locator('input[name=device]').fill('Hub design study — temporary read only');
  await page.locator('input[name=operator]').fill('Design study');
  await page.locator('#pair-form button[type=submit]').click();
  await page.locator('.machine-state.live').first().waitFor({timeout:15000});
  await page.waitForTimeout(4000); // Let the real first-connection toast settle.
  for (const count of [1,2]) {
    if (count===2) {
      // Same StoredMachine shape used by storage.ts; session sample adapted from
      // hub-web/fixtures/main.ts. Only the added host's transport is simulated.
      await page.evaluate(async () => {
        const keys=await crypto.subtle.generateKey({name:'ECDSA',namedCurve:'P-256'},true,['sign','verify']);
        const publicKey=await crypto.subtle.exportKey('jwk',keys.publicKey);
        const db=await new Promise((ok,bad)=>{const req=indexedDB.open('cas-commander-v1',1);req.onsuccess=()=>ok(req.result);req.onerror=()=>bad(req.error)});
        await new Promise((ok,bad)=>{const tx=db.transaction('machines','readwrite');tx.objectStore('machines').put({id:'forge-fixture',label:'Forge desktop (simulated)',baseUrl:'https://forge.invalid',deviceId:'study-fixture',credentialId:'fixture-only',credential:'fixture-only',expiresAt:'2099-01-01T00:00:00Z',scopes:['machine-read','session-read','pane-read'],publicKey,privateKey:keys.privateKey});tx.oncomplete=ok;tx.onerror=()=>bad(tx.error)});
        db.close();
      });
      await context.addInitScript(() => {
        const realFetch=window.fetch.bind(window);
        window.fetch=async (input,init)=>{
          const url=new URL(typeof input==='string'?input:input.url??String(input),location.href);
          if(url.hostname!=='forge.invalid')return realFetch(input,init);
          if(url.pathname==='/v1/events')return new Response(new ReadableStream({start(c){c.enqueue(new TextEncoder().encode(': fixture stream\n\n'))}}),{headers:{'Content-Type':'text/event-stream'}});
          const data=url.pathname==='/v1/sessions'?{sessions:[{name:'Memory relevance audit',supervisor:'quiet-marten',workers:['worker-one','worker-two'],liveness:'live'}]}:url.pathname==='/v1/machine'?{schema_version:1,version:'fixture',capabilities:[],transport:{mode:'fixture'}}:{};
          return new Response(JSON.stringify(data),{headers:{'Content-Type':'application/json'}});
        };
      });
      await page.reload();
      await page.getByText('2 machines', {exact:false}).first().waitFor({timeout:15000});
      await page.waitForTimeout(1500);
    }
    for(const scheme of ['light','dark']) {
      await page.emulateMedia({colorScheme:scheme});
      for(const viewport of [{name:'phone',width:390,height:844},{name:'desktop',width:1280,height:800}]) {
        await page.setViewportSize({width:viewport.width,height:viewport.height});
        await page.waitForTimeout(450);
        const file=`current-${count}-${viewport.name}-${scheme}.png`;
        await page.screenshot({path:resolve(outputDir,file)});
        const live=await page.locator('.machine-state.live').count();
        const text=await page.locator('body').innerText();
        await writeFile(resolve(outputDir,file.replace('.png','.txt')),text);
        receipts.push({file,systems:count,viewport,scheme,liveIndicators:live,capturedAt:new Date().toISOString(),evidence:count===1?'real-build; actual paired local Hub':'real-build UI; one actual Hub plus one explicitly simulated transport',distRevision:'3afcccce',sourceRevision:'a616f6d506a738557cd8977d130db56afa520d0d'});
      }
    }
  }
  await writeFile(resolve(outputDir,'manifest.json'),JSON.stringify(receipts,null,2)+'\n');
  console.log(`Captured ${receipts.length} current-state views; ${receipts[0].liveIndicators} actual live indicator(s). Second system is simulated.`);
} finally {await browser.close()}
