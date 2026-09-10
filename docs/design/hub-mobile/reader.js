// Progressive enhancement: static content and images remain readable without JS.
(() => {
  const study=document.querySelector('#study');
  const viewer=document.querySelector('#viewer');
  const viewport=document.querySelector('#viewport');
  const canvas=document.querySelector('#canvas');
  const image=document.querySelector('#viewer-image');
  const title=document.querySelector('#viewer-title');
  const caption=document.querySelector('#viewer-caption');
  const zoomLabel=document.querySelector('#zoom-level');
  const theme=document.querySelector('#theme');
  const scheme=matchMedia('(prefers-color-scheme: dark)');
  let opener, source, zoom=1, fit=1, manualScheme, fullTarget, savedStudyY=0;
  let measuredWidth=0, measuredHeight=0;
  let positioned=false, resizeFrame, scrollFraction={x:0,y:0};
  const points=new Map();
  let gesture;
  const MIN_ZOOM=.25, MAX_ZOOM=5;
  const clamp=(n,a,b)=>Math.min(b,Math.max(a,n));
  const currentScheme=()=>manualScheme||(scheme.matches?'dark':'light');
  function applyScheme(){
    const next=currentScheme();
    document.documentElement.dataset.scheme=next;
    document.querySelectorAll('picture source[data-dark]').forEach(s=>s.media=next==='dark'?'all':'not all');
    theme.textContent=next==='dark'?'Light view':'Dark view';
    theme.setAttribute('aria-label',`Switch to ${next==='dark'?'light':'dark'} appearance`);
    if(source?.dataset.current)image.src=next==='dark'?source.closest('picture').querySelector('source').srcset:source.src;
  }
  theme.addEventListener('click',()=>{manualScheme=currentScheme()==='dark'?'light':'dark';applyScheme()});
  scheme.addEventListener('change',()=>{if(!manualScheme)applyScheme()});
  function rememberPosition(){
    if(!positioned||viewport.clientWidth!==measuredWidth||viewport.clientHeight!==measuredHeight)return;
    scrollFraction={x:viewport.scrollLeft/Math.max(1,viewport.scrollWidth-viewport.clientWidth),y:viewport.scrollTop/Math.max(1,viewport.scrollHeight-viewport.clientHeight)};
  }
  function layout(anchor){
    const nativeWidth=Number(source?.dataset.width)||image.naturalWidth||390;
    fit=Math.max(1,viewport.clientWidth-32)/nativeWidth;
    const width=nativeWidth*fit*zoom;
    canvas.style.width=`${width}px`;
    canvas.style.marginInline=width<viewport.clientWidth-32?'auto':'0';
    zoomLabel.value=`${Math.round(zoom*100)}% of width`;
    zoomLabel.textContent=zoomLabel.value;
    if(anchor){viewport.scrollLeft=anchor.x*width-anchor.localX;viewport.scrollTop=anchor.y*image.clientHeight-anchor.localY;}
    viewer.dataset.zoom=String(zoom);
    measuredWidth=viewport.clientWidth;measuredHeight=viewport.clientHeight;
  }
  function zoomAt(next,clientX,clientY){
    const box=image.getBoundingClientRect(),area=viewport.getBoundingClientRect();
    const px=clientX??area.left+area.width/2, py=clientY??area.top+area.height/2;
    const anchor={x:(px-box.left)/box.width,y:(py-box.top)/box.height,localX:px-area.left-16,localY:py-area.top-16};
    zoom=clamp(next,MIN_ZOOM,MAX_ZOOM);layout(anchor);rememberPosition();
  }
  function reflow(){
    if(!viewer.open)return;
    const retained={...scrollFraction};
    positioned=false;
    layout();
    viewport.scrollLeft=retained.x*Math.max(0,viewport.scrollWidth-viewport.clientWidth);
    viewport.scrollTop=retained.y*Math.max(0,viewport.scrollHeight-viewport.clientHeight);
    requestAnimationFrame(()=>{positioned=true;rememberPosition()});
  }
  function scheduleReflow(){cancelAnimationFrame(resizeFrame);resizeFrame=requestAnimationFrame(reflow)}
  new ResizeObserver(scheduleReflow).observe(viewport);
  window.addEventListener('orientationchange',scheduleReflow);
  window.visualViewport?.addEventListener('resize',scheduleReflow);
  viewport.addEventListener('scroll',rememberPosition,{passive:true});
  document.querySelectorAll('[data-open-image]').forEach(button=>button.addEventListener('click',()=>{
    opener=button;source=button.querySelector('img');zoom=1;positioned=false;scrollFraction={x:0,y:0};
    title.textContent=button.dataset.title;
    caption.textContent=button.closest('figure').querySelector('figcaption').textContent;
    image.alt=source.alt;
    image.src=source.dataset.current?(currentScheme()==='dark'?source.closest('picture').querySelector('source').srcset:source.src):source.currentSrc||source.src;
    viewer.showModal();document.body.classList.add('image-open');
    image.decode().catch(()=>{}).then(()=>{layout();viewport.scrollTop=0;viewport.scrollLeft=0;positioned=true;document.querySelector('#close-viewer').focus()});
  }));
  function resetFullState(){
    if(!fullTarget)return;
    const wasStudy=fullTarget===study, restoreY=study.scrollTop;
    fullTarget.classList.remove('full-viewport');
    fullTarget.dataset.fullscreen='off';
    fullTarget=null;
    document.querySelectorAll('button[data-fullscreen]').forEach(b=>{b.textContent='Full screen';b.setAttribute('aria-pressed','false')});
    document.querySelector('#fullscreen-status').textContent='';
    document.body.classList.remove('study-open');
    if(wasStudy)requestAnimationFrame(()=>window.scrollTo(0,restoreY||savedStudyY));
    scheduleReflow();
  }
  async function exitFull(){
    if(document.fullscreenElement){try{await document.exitFullscreen()}catch{}}
    resetFullState();
  }
  async function enterFull(target){
    if(fullTarget===target){await exitFull();return}
    if(fullTarget)await exitFull();
    if(target===study){savedStudyY=window.scrollY;document.body.classList.add('study-open')}
    fullTarget=target;target.classList.add('full-viewport');target.dataset.fullscreen='fallback';
    if(target===study)study.scrollTop=savedStudyY;
    const button=target===study?document.querySelector('#study-fullscreen'):document.querySelector('#image-fullscreen');
    button.textContent='Exit full screen';button.setAttribute('aria-pressed','true');
    try {const nativeTarget=target===viewer?document.querySelector('#viewer-frame'):target;if(!nativeTarget.requestFullscreen)throw new Error('API unavailable');await nativeTarget.requestFullscreen();target.dataset.fullscreen='native'}
    catch {document.querySelector('#fullscreen-status').textContent='Full-viewport reader · browser fullscreen unavailable'}
    scheduleReflow();
  }
  document.querySelector('#study-fullscreen').addEventListener('click',()=>enterFull(study));
  document.querySelector('#image-fullscreen').addEventListener('click',()=>enterFull(viewer));
  document.addEventListener('fullscreenchange',()=>{if(!document.fullscreenElement&&fullTarget?.dataset.fullscreen==='native')resetFullState()});
  async function closeViewer(){if(fullTarget===viewer)await exitFull();viewer.close();document.body.classList.remove('image-open');points.clear();opener?.focus({preventScroll:true})}
  document.querySelector('#close-viewer').addEventListener('click',closeViewer);
  viewer.addEventListener('cancel',event=>{event.preventDefault();if(fullTarget)void exitFull();else void closeViewer()});
  document.addEventListener('keydown',event=>{
    if(event.key==='Escape'&&fullTarget){event.preventDefault();void exitFull()}
    if(!viewer.open)return;
    if(['+','=','-','0'].includes(event.key)){event.preventDefault();if(event.key==='0'){zoom=1;layout();rememberPosition()}else zoomAt(zoom*(event.key==='-'?1/1.25:1.25))}
  });
  document.querySelector('#zoom-in').addEventListener('click',()=>zoomAt(zoom*1.25));
  document.querySelector('#zoom-out').addEventListener('click',()=>zoomAt(zoom/1.25));
  document.querySelector('#fit-width').addEventListener('click',()=>{zoom=1;layout();rememberPosition()});
  viewport.addEventListener('wheel',event=>{event.preventDefault();zoomAt(zoom*Math.exp(-event.deltaY*.002),event.clientX,event.clientY)},{passive:false});
  function startGesture(){
    const p=[...points.values()];
    if(p.length===2)gesture={kind:'pinch',distance:Math.hypot(p[1].x-p[0].x,p[1].y-p[0].y),zoom};
    else if(p.length===1)gesture={kind:'pan',x:p[0].x,y:p[0].y,left:viewport.scrollLeft,top:viewport.scrollTop};
    else gesture=null;
  }
  viewport.addEventListener('pointerdown',event=>{if(event.pointerType==='mouse'&&event.button!==0)return;points.set(event.pointerId,{x:event.clientX,y:event.clientY});viewport.setPointerCapture(event.pointerId);startGesture();viewport.focus({preventScroll:true})});
  viewport.addEventListener('pointermove',event=>{
    if(!points.has(event.pointerId))return;points.set(event.pointerId,{x:event.clientX,y:event.clientY});const p=[...points.values()];
    if(gesture?.kind==='pinch'&&p.length===2){const dist=Math.hypot(p[1].x-p[0].x,p[1].y-p[0].y);zoomAt(gesture.zoom*dist/Math.max(1,gesture.distance),(p[0].x+p[1].x)/2,(p[0].y+p[1].y)/2)}
    else if(gesture?.kind==='pan'&&p.length===1){viewport.scrollLeft=gesture.left+gesture.x-p[0].x;viewport.scrollTop=gesture.top+gesture.y-p[0].y;rememberPosition()}
  });
  for(const event of ['pointerup','pointercancel','lostpointercapture'])viewport.addEventListener(event,e=>{points.delete(e.pointerId);startGesture()});
  image.addEventListener('dragstart',event=>event.preventDefault());
  // In the lightbox, wheel zooms; touch drag, the scrollbar and arrow/PageDown keys scroll.
  applyScheme();
})();
