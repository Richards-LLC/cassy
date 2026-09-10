import {launchBrowser} from './browser-tools.mjs';
import {proposalDrawing} from './drawings.mjs';
import {writeFile,mkdir} from 'node:fs/promises';
const out=process.argv[2]||'/home/pippenz/.cas/artifacts/cas-cc1b';
await mkdir(out,{recursive:true});
const browser=await launchBrowser();const page=await browser.newPage();let defects=[];
try{for(let n=1;n<=10;n++)for(const type of ['phone','desktop']){
await page.setViewportSize({width:type==='phone'?390:1280,height:type==='phone'?844:800});await page.setContent('<style>body{margin:0}</style>'+proposalDrawing(n,type));
const checks=await page.evaluate(()=>{const svg=document.querySelector('svg'),{width,height}=svg.viewBox.baseVal;const rects=[...svg.querySelectorAll('text')].map(t=>{const r=t.getBBox();return{text:t.textContent,x:r.x,y:r.y,w:r.width,h:r.height}});const out=rects.filter(r=>r.x<0||r.y<0||r.x+r.w>width||r.y+r.h>height).map(r=>({type:'outside',...r}));for(let i=0;i<rects.length;i++)for(let j=i+1;j<rects.length;j++){const a=rects[i],b=rects[j];if(Math.min(a.x+a.w,b.x+b.w)-Math.max(a.x,b.x)>1&&Math.min(a.y+a.h,b.y+b.h)-Math.max(a.y,b.y)>1)out.push({type:'text-overlap',a:a.text,b:b.text})}return out});defects.push(...checks.map(x=>({n,type,...x})));
await page.screenshot({path:`${out}/drawing-${n}-${type}.png`});
}console.log(defects.length?JSON.stringify(defects,null,2):"PASS 20 SVG views: no text overlaps or out-of-bounds labels");process.exitCode=defects.length?1:0;await writeFile(`${out}/drawing-checks.json`,JSON.stringify({count:20,defects},null,2));}finally{await browser.close()}
