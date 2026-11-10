// Real AudioWorklet/MediaStream; native IPC supplies synthetic PCM.
import assert from 'node:assert/strict';
import {createRequire} from 'node:module';
import {createServer} from 'node:http';
import {writeFile} from 'node:fs/promises';
import {build} from 'esbuild';
const {chromium}=createRequire(import.meta.url)(process.env.PLAYWRIGHT_MODULE||'playwright');
const {outputFiles}=await build({stdin:{contents:`import {startNativeScreenAudio} from './src/nativeScreenAudio';window.begin=async()=>{window.capture=await startNativeScreenAudio({server:'test',session:'1',channel:'1'},()=>window.failed=true);const context=new AudioContext();await context.resume();window.analyser=context.createAnalyser();context.createMediaStreamSource(new MediaStream([window.capture.track])).connect(window.analyser);window.energy=()=>{const a=new Float32Array(window.analyser.fftSize);window.analyser.getFloatTimeDomainData(a);return a.reduce((s,n)=>s+n*n,0)/a.length;};};`,resolveDir:process.cwd(),loader:'ts'},bundle:true,write:false,format:'iife',plugins:[{name:'fake-ipc',setup(b){b.onResolve({filter:/^@tauri-apps\/api\/core$/},()=>({path:'ipc',namespace:'test'}));b.onLoad({filter:/.*/,namespace:'test'},()=>({contents:`let phase=0;export async function invoke(command,args){if(command==='set_screen_audio_paused'){window.nativePaused=args.paused;return;}if(command==='start_screen_audio')return 48000;if(command==='stop_screen_audio'){window.stopped=(window.stopped||0)+1;return;}if(window.nativePaused)return [];if(window.breakIPC)throw new Error('capture stopped');const data=[];for(let i=0;i<960;i++){const v=Math.sin(phase++*2*Math.PI*440/48000)*.2;data.push(v,v);}return data;}`,loader:'js'}));}}]});
if(process.argv.includes('--webkit')){
 const test=`const wait=async(check)=>{const end=Date.now()+3000;while(!check()){if(Date.now()>end)throw new Error('audio assertion timeout');await new Promise(r=>setTimeout(r,30));}};
 (async()=>{try{await window.begin();await wait(()=>window.energy()>.005);const energy=window.energy();
 window.capture.track.enabled=false;await wait(()=>window.energy()<1e-8);
 window.capture.track.enabled=true;await wait(()=>window.energy()>.005);
 window.breakIPC=true;await wait(()=>window.failed&&window.stopped===1);
 if(window.capture.track.readyState!=='ended')throw new Error('track not released');
 webkit.messageHandlers.result.postMessage(JSON.stringify({ok:true,engine:'WKWebView',energy,muteResume:true,cleanup:true}));
 }catch(e){webkit.messageHandlers.result.postMessage(JSON.stringify({ok:false,error:String(e)}));}})();`;
 await writeFile('/tmp/eden-audio-worklet-webkit.html','<!doctype html><meta charset="utf-8"><script>'+outputFiles[0].text+test+'</script>');
 console.log('/tmp/eden-audio-worklet-webkit.html');process.exit(0);
}
const server=createServer((req,res)=>{res.setHeader('Content-Type','text/html');res.end('<!doctype html><title>PCM test</title>');});
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));let browser;
try{
 browser=await chromium.launch({headless:true,executablePath:process.env.CHROME_EXECUTABLE,args:['--autoplay-policy=no-user-gesture-required']});
 const page=await browser.newPage();const errors=[];page.on('pageerror',e=>errors.push(e.message));await page.goto(`http://127.0.0.1:${server.address().port}`);
 await page.addScriptTag({content:outputFiles[0].text});await page.evaluate(()=>window.begin());
 await page.waitForFunction(()=>window.energy()>.005);
 await page.evaluate(()=>window.capture.track.enabled=false);await page.waitForFunction(()=>window.energy()<1e-8);
 await page.evaluate(()=>window.capture.track.enabled=true);await page.waitForFunction(()=>window.energy()>.005);
 await page.evaluate(()=>window.capture.pause(true));await page.waitForFunction(()=>window.energy()<1e-8);assert.equal(await page.evaluate(()=>window.nativePaused),true);
 // Even a consumer accidentally enabling the track cannot replay queued PCM.
 await page.evaluate(()=>window.capture.track.enabled=true);await page.waitForTimeout(120);assert.ok(await page.evaluate(()=>window.energy()<1e-8));
 await page.evaluate(()=>window.capture.pause(false));assert.equal(await page.evaluate(()=>window.capture.track.enabled),false);await page.evaluate(()=>window.capture.track.enabled=true);await page.waitForFunction(()=>window.energy()>.005);
 await page.evaluate(()=>window.breakIPC=true);await page.waitForFunction(()=>window.failed&&window.stopped===1);
 assert.equal(await page.evaluate(()=>window.capture.track.readyState),'ended');await page.evaluate(()=>window.capture.stop());assert.equal(await page.evaluate(()=>window.stopped),1);
 assert.deepEqual(errors,[]);console.log('PASS: PCM worklet tone, mute/resume, privacy gate + queue purge, IPC failure cleanup, idempotent stop');
}finally{await browser?.close();await new Promise(resolve=>server.close(resolve));}
