import assert from 'node:assert/strict';
import test from 'node:test';
import { build } from 'esbuild';
const { outputFiles } = await build({entryPoints:['src/screenShareController.ts'], bundle:true, write:false, format:'esm', platform:'node'});
const {ScreenShareController,fitScreenSettings,defaultScreenSettings,screenShareError}=await import(`data:text/javascript;base64,${Buffer.from(outputFiles[0].text).toString('base64')}`);
const policy={enabled:true,max_width:1280,max_height:720,max_fps:30,max_bitrate_kbps:2000,allow_audio:false,max_viewers:8,max_rooms:4};
const context={server:'localhost',session:'1',channel:'7'};
const state={channel_id:'7',host_id:'2',share_id:'share',settings:defaultScreenSettings,policy,viewers:0,ready:true,action:'state',server_address:'localhost',self_session_id:'1'};
function controller(){const sent=[];let c; c=new ScreenShareController(context,async request=>{sent.push(request);queueMicrotask(()=>c.receive({...state,action:request.action,request_id:request.request_id}));});return {c,sent};}
test('server ceilings constrain every requested quality parameter and audio',()=>{
 assert.deepEqual(fitScreenSettings({width:3840,height:2160,fps:60,bitrate_kbps:10000,audio:true},policy),{...defaultScreenSettings});
});
test('foreign connection and room events never replace current share',()=>{
 const {c}=controller();c.receive(state);
 for(const override of [{server_address:'other'},{self_session_id:'9'},{channel_id:'8'}])c.receive({...state,...override,share_id:'wrong'});
 assert.equal(c.view.room.share_id,'share');c.dispose();
});
test('cancel while subscribing removes only own negotiation, never stops another host',async()=>{
 const {c,sent}=controller();c.receive(state);c.view.busy=true;
 await c.stop();assert.equal(sent[0].action,'cancel');assert.ok(!sent.some(r=>r.action==='stop'));c.dispose();
});
test('capture returned after disposal immediately stops every track',async()=>{
 let resolveCapture;let stopped=0;
 const original=Object.getOwnPropertyDescriptor(globalThis,'navigator');
 Object.defineProperty(globalThis,'navigator',{configurable:true,value:{mediaDevices:{getDisplayMedia:()=>new Promise(resolve=>{resolveCapture=resolve;})}}});
 try{
  const {c}=controller();c.receive({...state,share_id:'',host_id:'0',ready:false});
  const pending=c.publish(defaultScreenSettings,'monitor');c.dispose();
  resolveCapture({getTracks:()=>[{stop:()=>stopped++},{stop:()=>stopped++}]});await pending;
  assert.equal(stopped,2);assert.equal(c.view.stream,null);assert.equal(c.view.role,'idle');
 }finally{if(original)Object.defineProperty(globalThis,'navigator',original);else delete globalThis.navigator;}
});
test('late publish response cannot resurrect an already stopped share',()=>{
 const {c}=controller();
 c.receive({...state,revision:'9007199254740992'});
 c.receive({...state,revision:'9007199254740994',share_id:'',ready:false});
 c.receive({...state,revision:'9007199254740993',action:'publish'});
 assert.equal(c.view.room.share_id,'');assert.equal(c.view.role,'idle');c.dispose();
});
test('late status RPC resolves without overwriting a newer broadcast',async()=>{
 let request;const c=new ScreenShareController(context,async r=>{request=r;});
 c.receive({...state,revision:'1'});
 const refresh=c.refresh();
 c.receive({...state,revision:'3',share_id:'new-share'});
 c.receive({...state,revision:'2',action:'status',request_id:request.request_id});
 await refresh;assert.equal(c.view.room.share_id,'new-share');c.dispose();
});

test('quality inputs stay finite integers within supported bounds',()=>{
 const v=fitScreenSettings({width:NaN,height:-1,fps:29.6,bitrate_kbps:Infinity,audio:true},policy);
 assert.deepEqual(v,{width:1280,height:90,fps:30,bitrate_kbps:2000,audio:false});
});
test('capture errors explain cancellation and system constraints without hiding signaling failures',()=>{
 assert.match(screenShareError({name:'NotAllowedError'}),/取消选择/);
 assert.match(screenShareError({name:'OverconstrainedError'}),/降低分辨率或帧率/);
 assert.equal(screenShareError(new Error('server limit changed')),'server limit changed');
});

test('OS stop during pending capture constraints promptly releases video and audio',async()=>{
 let finishConstraints;let videoStops=0,audioStops=0;
 const video={readyState:'live',stop(){videoStops++;},applyConstraints:()=>new Promise(resolve=>{finishConstraints=resolve;})};
 const audio={stop(){audioStops++;}};
 const stream={getTracks:()=>[video,audio],getVideoTracks:()=>[video],getAudioTracks:()=>[audio]};
 const original=Object.getOwnPropertyDescriptor(globalThis,'navigator');
 Object.defineProperty(globalThis,'navigator',{configurable:true,value:{mediaDevices:{getDisplayMedia:async()=>stream}}});
 const {c,sent}=controller();
 try{
  c.receive({...state,share_id:'',host_id:'0',ready:false});
  const pending=c.publish(defaultScreenSettings,'window');await Promise.resolve();
  assert.equal(typeof video.onended,'function');video.readyState='ended';video.onended();
  assert.equal(videoStops,1);assert.equal(audioStops,1);assert.equal(c.view.busy,false);
  finishConstraints();await pending;
  assert.equal(c.view.role,'idle');assert.ok(!sent.some(r=>r.action==='publish'));
 }finally{c.dispose();if(original)Object.defineProperty(globalThis,'navigator',original);else delete globalThis.navigator;}
});

test('an already ended capture never starts a media negotiation',async()=>{
 let stops=0,applied=false;
 const video={readyState:'ended',stop(){stops++;},async applyConstraints(){applied=true;}};
 const original=Object.getOwnPropertyDescriptor(globalThis,'navigator');
 Object.defineProperty(globalThis,'navigator',{configurable:true,value:{mediaDevices:{getDisplayMedia:async()=>({getTracks:()=>[video],getVideoTracks:()=>[video]})}}});
 const {c,sent}=controller();
 try{
  c.receive({...state,share_id:'',host_id:'0',ready:false});await c.publish(defaultScreenSettings,'window');
  assert.equal(stops,1);assert.equal(applied,false);assert.match(c.view.error,/已停止/);assert.ok(!sent.some(r=>r.action==='publish'));
 }finally{c.dispose();if(original)Object.defineProperty(globalThis,'navigator',original);else delete globalThis.navigator;}
});

test('native audio returned after room disposal is stopped instead of attached',async()=>{
 let resolveAudio;let stopped=0;
 const video={readyState:'live',stop(){},async applyConstraints(){}};
 const stream={getTracks:()=>[video],getVideoTracks:()=>[video],getAudioTracks:()=>[],addTrack(){throw new Error('late track attached');}};
 const original=Object.getOwnPropertyDescriptor(globalThis,'navigator');
 Object.defineProperty(globalThis,'navigator',{configurable:true,value:{mediaDevices:{getDisplayMedia:async()=>stream}}});
 const c=new ScreenShareController(context,async()=>{},()=>new Promise(resolve=>{resolveAudio=resolve;}));
 try{
  c.receive({...state,policy:{...policy,allow_audio:true},share_id:'',ready:false});
  const pending=c.publish({...defaultScreenSettings,audio:true},'system');
  await Promise.resolve();await Promise.resolve();assert.equal(typeof resolveAudio,'function');
  c.dispose();resolveAudio({track:{},stop(){stopped++;}});await pending;
  assert.equal(stopped,1);assert.equal(c.view.role,'idle');
 }finally{c.dispose();if(original)Object.defineProperty(globalThis,'navigator',original);else delete globalThis.navigator;}
});

test('older WebView2 without codec preference API uses server SDP negotiation',()=>{
 const original=globalThis.RTCRtpSender;
 try{
  globalThis.RTCRtpSender={getCapabilities:()=>({codecs:[{mimeType:'video/VP8'}]})};
  const {c}=controller();assert.doesNotThrow(()=>c.videoCodec({}));
  let codecs;c.videoCodec({setCodecPreferences:v=>codecs=v});assert.equal(codecs[0].mimeType,'video/VP8');
  globalThis.RTCRtpSender={};assert.doesNotThrow(()=>c.videoCodec({}));c.dispose();
 }finally{globalThis.RTCRtpSender=original;}
});

test('ICE cancellation and failed local description remove negotiation listeners',async()=>{
 const {c}=controller();const listeners=new Set();
 const pc={iceGatheringState:'gathering',addEventListener:(_,f)=>listeners.add(f),removeEventListener:(_,f)=>listeners.delete(f),createOffer:async()=>({}),setLocalDescription:async()=>{}};
 const pending=c.offer(pc);await Promise.resolve();await Promise.resolve();
 c.release();await assert.rejects(pending,/已取消/);assert.equal(listeners.size,0);
 pc.setLocalDescription=async()=>{throw new Error('local description failed');};
 await assert.rejects(c.offer(pc),/local description failed/);assert.equal(listeners.size,0);c.dispose();
});

test('already completed ICE resolves even if no new gathering event fires',async()=>{
 const {c}=controller();const listeners=new Set();
 const pc={iceGatheringState:'complete',localDescription:{sdp:'complete SDP'},addEventListener:(_,f)=>listeners.add(f),removeEventListener:(_,f)=>listeners.delete(f),createOffer:async()=>({}),setLocalDescription:async()=>{}};
 assert.equal(await c.offer(pc),'complete SDP');assert.equal(listeners.size,0);c.dispose();
});

test('connection deadline releases capture; brief disconnect recovery cancels deadline',async()=>{
 const originalPeer=globalThis.RTCPeerConnection,originalSet=globalThis.setTimeout,originalClear=globalThis.clearTimeout;
 const timers=new Map();let next=0;
 globalThis.setTimeout=(fn,ms)=>{const id=++next;timers.set(id,{fn,ms});return id;};
 globalThis.clearTimeout=id=>timers.delete(id);
 globalThis.RTCPeerConnection=class{connectionState='new';closed=false;close(){this.closed=true;}};
 let c;
 try{
  ({c}=controller());c.receive(state);
  let captureStops=0;c.local={getTracks:()=>[{stop(){captureStops++;}}]};c.view.role='viewing';
  const pc=c.peer(c.generation);
  assert.ok([...timers.values()].some(t=>t.ms===20000));
  pc.connectionState='connected';pc.onconnectionstatechange();assert.equal(timers.size,0);
  pc.connectionState='disconnected';pc.onconnectionstatechange();assert.equal(timers.size,1);assert.equal([...timers.values()][0].ms,10000);
  const stale=[...timers.values()][0].fn;
  pc.connectionState='connected';pc.onconnectionstatechange();assert.equal(timers.size,0);
  assert.equal(captureStops,0);assert.equal(pc.closed,false);
  pc.connectionState='disconnected';pc.onconnectionstatechange();const timeout=[...timers.values()][0].fn;timeout();
  assert.equal(pc.closed,true);assert.equal(captureStops,1);assert.equal(c.view.role,'idle');assert.match(c.view.error,/连接中断/);
  stale();assert.equal(captureStops,1);
  await Promise.resolve();await Promise.resolve();
 }finally{c?.dispose();globalThis.RTCPeerConnection=originalPeer;globalThis.setTimeout=originalSet;globalThis.clearTimeout=originalClear;}
});

function privacyController({replaceFailure=false,pauseFailure=false,nativeFailure=false}={}){
 const sent=[],replaced=[],nativePauses=[];let c;
 c=new ScreenShareController(context,async request=>{sent.push(request);queueMicrotask(()=>c.receive({...state,paused:request.paused??false,action:request.action,request_id:request.request_id,error:request.action==='pause'&&pauseFailure?'signaling failed':undefined}));});
 const video={kind:'video',readyState:'live',enabled:true,stop(){this.readyState='ended';}},audio={kind:'audio',readyState:'live',enabled:true,stop(){this.readyState='ended';}};
 const privacyTrack={kind:'video',readyState:'live',stop(){this.readyState='ended';}};
 const stream={getTracks:()=>[video,audio],getVideoTracks:()=>[video],getAudioTracks:()=>[audio]};
 c.receive({...state,paused:false});c.local=stream;c.view={...c.view,role:'publishing',stream};
 c.videoSender={async replaceTrack(track){replaced.push(track);if(replaceFailure)throw new Error('replace failed');}};
 c.nativeAudio={track:audio,stop(){audio.stop();},async pause(value){nativePauses.push(value);if(nativeFailure)throw new Error('native gate failed');}};
 c.privacyStream=()=>({stream:{getTracks:()=>[privacyTrack],getVideoTracks:()=>[privacyTrack]},stop(){privacyTrack.stop();}});
 return {c,sent,replaced,nativePauses,video,audio,privacyTrack,stream};
}
test('privacy pause gates real tracks synchronously, then sends safe media before metadata; resumes the same source and audio preference',async()=>{
 const {c,sent,replaced,nativePauses,video,audio,privacyTrack,stream}=privacyController();
 try{
  const pause=c.setPrivacyPaused(true);
  assert.equal(video.enabled,false);assert.equal(audio.enabled,false);assert.equal(c.view.privacyPaused,true);assert.equal(sent.length,0);
  await pause;assert.equal(replaced[0],privacyTrack);assert.deepEqual(nativePauses,[true]);assert.equal(sent[0].action,'pause');assert.equal(sent[0].paused,true);
  c.setSharedAudio(false);await c.setPrivacyPaused(false);
  assert.equal(c.view.stream,stream);assert.equal(replaced[1],video);assert.equal(video.enabled,true);assert.equal(audio.enabled,false);assert.equal(privacyTrack.readyState,'ended');assert.deepEqual(nativePauses,[true,false]);assert.equal(c.view.privacyPaused,false);
 }finally{c.dispose();}
});
test('failed pause signaling stays private with a visible error, never reenables captured media',async()=>{
 const {c,video,audio,privacyTrack}=privacyController({pauseFailure:true});
 try{await c.setPrivacyPaused(true);assert.equal(c.view.role,'publishing');assert.equal(c.view.privacyPaused,true);assert.equal(video.enabled,false);assert.equal(audio.enabled,false);assert.equal(privacyTrack.readyState,'live');assert.match(c.view.error,/同步失败/);}finally{c.dispose();}
});
test('failed privacy canvas replacement or native gate stops the entire share safely',async()=>{
 for(const options of [{replaceFailure:true},{nativeFailure:true}]){
  const {c,video,audio,privacyTrack,sent}=privacyController(options);
  try{await c.setPrivacyPaused(true);assert.equal(c.view.role,'idle');assert.equal(video.readyState,'ended');assert.equal(audio.readyState,'ended');assert.equal(privacyTrack.readyState,'ended');assert.ok(sent.some(r=>r.action==='stop'));assert.match(c.view.error,/共享已停止/);}finally{c.dispose();}
 }
});
test('resume refusal stays paused; enabling shared audio while paused cannot send sound',async()=>{
 const {c,video,audio}=privacyController();
 try{
  await c.setPrivacyPaused(true);c.setSharedAudio(true);assert.equal(audio.enabled,false);
  c.send=async request=>queueMicrotask(()=>c.receive({...state,request_id:request.request_id,action:request.action,paused:true,error:request.paused===false?'resume denied':undefined}));
  await c.setPrivacyPaused(false);assert.equal(c.view.privacyPaused,true);assert.equal(video.enabled,false);assert.equal(audio.enabled,false);assert.match(c.view.error,/未能恢复/);
 }finally{c.dispose();}
});
test('room disposal racing privacy track replacement releases both original and safe tracks',async()=>{
 const {c,video,audio,privacyTrack,sent}=privacyController();let finish;
 c.videoSender={replaceTrack:()=>new Promise(resolve=>{finish=resolve;})};
 const operation=c.setPrivacyPaused(true);c.dispose();finish();await operation;
 assert.equal(video.readyState,'ended');assert.equal(audio.readyState,'ended');assert.equal(privacyTrack.readyState,'ended');assert.equal(c.view.role,'idle');assert.ok(!sent.some(r=>r.action==='pause'));
});
test('paused metadata is present for a late viewer, and old revisions cannot remove the placeholder',()=>{
 const {c}=controller();c.receive({...state,paused:true,revision:'2'});c.receive({...state,paused:false,revision:'1'});
 assert.equal(c.view.room.paused,true);c.receive({...state,share_id:'',paused:false,revision:'3'});assert.equal(c.view.room.paused,false);c.dispose();
});

test('old server snapshots cannot start privacy pause without explicit support',async()=>{
 const {c,video,audio,replaced,sent}=privacyController();delete c.view.room.paused;
 try{await c.setPrivacyPaused(true);assert.equal(video.enabled,true);assert.equal(audio.enabled,true);assert.equal(replaced.length,0);assert.equal(sent.length,0);assert.match(c.view.error,/尚未支持/);}finally{c.dispose();}
});

test('source replacement failure on resume regates native audio and keeps safe pixels',async()=>{
 const {c,video,audio,privacyTrack,nativePauses}=privacyController();
 try{
  await c.setPrivacyPaused(true);c.videoSender={async replaceTrack(){throw new Error('resume replacement failed');}};
  await c.setPrivacyPaused(false);assert.deepEqual(nativePauses,[true,false,true]);assert.equal(video.enabled,false);assert.equal(audio.enabled,false);assert.equal(privacyTrack.readyState,'live');assert.equal(c.view.privacyPaused,true);assert.match(c.view.error,/未能恢复/);
 }finally{c.dispose();}
});
test('room change during resume signaling never reenables the old capture',async()=>{
 const {c,video,audio}=privacyController();await c.setPrivacyPaused(true);
 const resume=c.setPrivacyPaused(false);c.dispose();await resume;
 assert.equal(video.enabled,false);assert.equal(audio.enabled,false);assert.equal(video.readyState,'ended');assert.equal(c.view.role,'idle');
});
