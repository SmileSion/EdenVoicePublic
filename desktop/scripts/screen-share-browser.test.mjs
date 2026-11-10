import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { build } from 'esbuild';
const require=createRequire(import.meta.url);
const {chromium}=require(process.env.PLAYWRIGHT_MODULE||'playwright');
const quality=JSON.parse(process.env.EDEN_SCREEN_QUALITY||'{"width":640,"height":360,"fps":10,"bitrate_kbps":1000}');
const url=process.argv[2];if(!url)throw new Error('Pass the local Go fixture URL');
const {outputFiles}=await build({entryPoints:['src/screenShareController.ts'],bundle:true,write:false,format:'iife',globalName:'ScreenModule'});
// Chromium's fake output device keeps Web Audio's clock moving without requiring
// working host speakers. Audio still traverses the real Web Audio/Opus/RTP path.
const browser=await chromium.launch({executablePath:process.env.CHROME_EXECUTABLE,headless:true,args:['--autoplay-policy=no-user-gesture-required','--disable-audio-output']});
const host=await browser.newPage(),viewer=await browser.newPage();
try {
 for(const [page,session] of [[host,'1'],[viewer,'2']]){
  page.on('pageerror',e=>console.error('browser:',e.message));
  await page.goto(url);await page.addScriptTag({content:outputFiles[0].text});
  await page.evaluate(session=>{
   window.c=new ScreenModule.ScreenShareController({server:'fixture',session,channel:'7'},async request=>{
    const res=await fetch(`/?viewer=${session==='2'?'1':'0'}`,{method:'POST',body:JSON.stringify(request)});c.receive(await res.json());
   });
   const video=document.createElement('video');video.autoplay=true;video.muted=session==='1';document.body.append(video);window.video=video;
   c.onChange(view=>{if(video.srcObject!==view.stream)video.srcObject=view.stream;if(view.stream)void video.play().catch(()=>{});});
   return c.refresh();
  },session);
 }
 await host.evaluate(async quality=>{
  const canvas=document.createElement('canvas');canvas.width=quality.width;canvas.height=quality.height;document.body.append(canvas);
  const ctx=canvas.getContext('2d');let frame=0;
  window.draw=setInterval(()=>{ctx.fillStyle=frame++%2?'#c32132':'#243ade';ctx.fillRect(0,0,canvas.width,canvas.height);ctx.fillStyle='white';ctx.fillText(String(frame),20,20);},1000/quality.fps);
  const audio=new AudioContext();window.audio=audio;const oscillator=audio.createOscillator();oscillator.frequency.value=440;const gain=audio.createGain();gain.gain.value=.1;const destination=audio.createMediaStreamDestination();oscillator.connect(gain).connect(destination);oscillator.start();await audio.resume();
  const stream=canvas.captureStream(quality.fps);stream.addTrack(destination.stream.getAudioTracks()[0]);window.capture=stream;
  window.captureCount=0;Object.defineProperty(navigator.mediaDevices,'getDisplayMedia',{value:async()=>{window.captureCount++;return stream;}});
  await c.publish({...quality,audio:true},'monitor');
  if(c.view.error)throw new Error(c.view.error);
 },quality);
 await host.waitForFunction(()=>audio.currentTime>.1,null,{timeout:3000,polling:50});
 for(let i=0;i<50;i++){const ready=await viewer.evaluate(async()=>{await c.refresh();return c.view.room?.ready;});if(ready)break;await new Promise(r=>setTimeout(r,100));}
 assert.equal(await viewer.evaluate(()=>c.view.room?.ready),true,'publisher ready');
 await viewer.evaluate(()=>c.watch());
 await viewer.waitForFunction(quality=>video.videoWidth===quality.width&&video.videoHeight===quality.height&&video.currentTime>.3,quality,{timeout:15000,polling:100});
 const pixels=await viewer.evaluate(()=>{
  const canvas=document.createElement('canvas');canvas.width=640;canvas.height=360;const ctx=canvas.getContext('2d');ctx.drawImage(video,0,0);return Array.from(ctx.getImageData(200,200,1,1).data);
 });
 assert.ok(Math.max(...pixels.slice(0,3))>120,`No decoded color: ${pixels}`);
 await viewer.evaluate(async()=>{
  const audio=new AudioContext();window.audio=audio;const source=audio.createMediaStreamSource(video.srcObject);window.analyser=audio.createAnalyser();source.connect(analyser);await audio.resume();
  window.energy=()=>{const data=new Float32Array(analyser.fftSize);analyser.getFloatTimeDomainData(data);return data.reduce((sum,n)=>sum+n*n,0)/data.length;};
 });
 await viewer.waitForFunction(()=>audio.currentTime>.1,null,{timeout:3000,polling:50});
 await viewer.waitForFunction(()=>energy()>.0001,null,{timeout:10000,polling:100});
 await host.evaluate(()=>c.setSharedAudio(false));
 await viewer.waitForFunction(()=>energy()<.000001,null,{timeout:10000,polling:100});
 await host.evaluate(()=>c.setSharedAudio(true));
 await viewer.waitForFunction(()=>energy()>.0001,null,{timeout:10000,polling:100});
 // Privacy is actual transmitted replacement media, independent of UI overlays.
 await host.evaluate(()=>c.setPrivacyPaused(true));assert.equal(await host.evaluate(()=>c.view.privacyPaused),true);
 assert.equal(await host.evaluate(()=>capture.getTracks().every(t=>!t.enabled)),true);
 await viewer.evaluate(()=>c.refresh());assert.equal(await viewer.evaluate(()=>c.view.room.paused),true);
 await viewer.waitForFunction(()=>energy()<.000001,null,{timeout:10000,polling:100});
 await viewer.waitForFunction(()=>{
  const cv=document.createElement('canvas');cv.width=video.videoWidth;cv.height=video.videoHeight;
  const ctx=cv.getContext('2d');ctx.drawImage(video,0,0);const p=ctx.getImageData(20,20,1,1).data;
  // VP8/WebView pipelines may expand studio-range RGB on decode. Accept both
  // color-range representations of the safe background, never the saturated
  // red/blue source frames or an undecoded black frame.
  const expected=[23,26,37];
  return [expected,expected.map(v=>(v-16)*255/219)].some(rgb=>rgb.every((v,i)=>Math.abs(p[i]-v)<8));
 },null,{timeout:10000,polling:100});
 const privacyPixels=await viewer.evaluate(()=>{const cv=document.createElement('canvas');cv.width=video.videoWidth;cv.height=video.videoHeight;const ctx=cv.getContext('2d');ctx.drawImage(video,0,0);return Array.from(ctx.getImageData(20,20,1,1).data);});
 // Rejoining while paused receives the same safe media and authoritative status.
 await viewer.evaluate(()=>c.stop());await viewer.evaluate(()=>c.refresh());assert.equal(await viewer.evaluate(()=>c.view.room.paused),true);await viewer.evaluate(()=>c.watch());
 await viewer.waitForFunction(()=>video.currentTime>.2,null,{timeout:15000,polling:100});
 assert.equal(await viewer.evaluate(()=>c.view.room.paused),true);
 await viewer.waitForFunction(()=>{
  const cv=document.createElement('canvas');cv.width=video.videoWidth;cv.height=video.videoHeight;
  const ctx=cv.getContext('2d');ctx.drawImage(video,0,0);const p=ctx.getImageData(20,20,1,1).data;
  // VP8/WebView pipelines may expand studio-range RGB on decode. Accept both
  // color-range representations of the safe background, never the saturated
  // red/blue source frames or an undecoded black frame.
  const expected=[23,26,37];
  return [expected,expected.map(v=>(v-16)*255/219)].some(rgb=>rgb.every((v,i)=>Math.abs(p[i]-v)<8));
 },null,{timeout:10000,polling:100});
 await viewer.evaluate(()=>{audio.createMediaStreamSource(video.srcObject).connect(analyser);});
 await viewer.waitForFunction(()=>energy()<.000001,null,{timeout:10000,polling:100});
 await host.evaluate(()=>c.setPrivacyPaused(false));await viewer.evaluate(()=>c.refresh());
 assert.equal(await host.evaluate(()=>c.view.privacyPaused),false);assert.equal(await viewer.evaluate(()=>c.view.room.paused),false);assert.equal(await host.evaluate(()=>captureCount),1,'resume retains original capture authorization');
 await viewer.waitForFunction(()=>energy()>.0001,null,{timeout:10000,polling:100});
 await viewer.waitForFunction(()=>{const cv=document.createElement('canvas');cv.width=video.videoWidth;cv.height=video.videoHeight;const ctx=cv.getContext('2d');ctx.drawImage(video,0,0);return Math.max(...ctx.getImageData(30,30,1,1).data.slice(0,3))>120;},null,{timeout:10000,polling:100});
 await viewer.waitForFunction(async()=>Array.from((await c.pc.getStats()).values()).some(s=>s.type==='inbound-rtp'&&s.kind==='audio'&&s.totalAudioEnergy>0),null,{timeout:10000,polling:100});
 const audioStats=await viewer.evaluate(async()=>Array.from((await c.pc.getStats()).values()).filter(s=>s.type==='inbound-rtp'&&s.kind==='audio').map(s=>({packetsReceived:s.packetsReceived,bytesReceived:s.bytesReceived,totalAudioEnergy:s.totalAudioEnergy})));
 assert.ok(audioStats.some(s=>s.packetsReceived>0&&s.bytesReceived>0&&s.totalAudioEnergy>0),'real audio RTP decoded');
 const videoStats=await viewer.evaluate(async()=>Array.from((await c.pc.getStats()).values()).filter(s=>s.type==='inbound-rtp'&&s.kind==='video').map(s=>({framesDecoded:s.framesDecoded,packetsLost:s.packetsLost,nackCount:s.nackCount,pliCount:s.pliCount})));
 await host.evaluate(()=>c.stop());
 assert.equal(await host.evaluate(()=>capture.getTracks().every(t=>t.readyState==='ended')),true);
 await viewer.evaluate(()=>c.refresh());
 assert.equal(await viewer.evaluate(()=>c.view.role),'idle');
 console.log(JSON.stringify({decodedVideo:`${quality.width}x${quality.height}`,requestedQuality:quality,videoStats,audioStats,pixels,privacy:{pixels:privacyPixels,pause:true,lateViewer:true,resumeWithoutRecapture:true},sharedAudio:'tone decoded; off/on/privacy verified',cleanup:'capture ended and viewer released'}));
} catch(error) {
 for(const [label,page] of [['host',host],['viewer',viewer]]) console.error(label,await page.evaluate(async()=>({decodedPixel:(()=>{if(!video.videoWidth)return null;const cv=document.createElement('canvas');cv.width=video.videoWidth;cv.height=video.videoHeight;const ctx=cv.getContext('2d');ctx.drawImage(video,0,0);return Array.from(ctx.getImageData(20,20,1,1).data);})(),audio:window.audio?{state:audio.state,time:audio.currentTime,sampleRate:audio.sampleRate}:null,tracks:window.capture?.getTracks().map(t=>({kind:t.kind,enabled:t.enabled,muted:t.muted,ready:t.readyState})),energy:window.energy?.(),view:{role:c.view.role,error:c.view.error,ready:c.view.room?.ready,share:c.view.room?.share_id},connection:c.pc?.connectionState,ice:c.pc?.iceConnectionState,stats:c.pc?Array.from((await c.pc.getStats()).values()).filter(s=>['outbound-rtp','inbound-rtp','candidate-pair','local-candidate','remote-candidate'].includes(s.type)):[]})));
 throw error;
} finally {await browser.close();}
