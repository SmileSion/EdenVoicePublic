export interface ScreenSettings { width: number; height: number; fps: number; bitrate_kbps: number; audio: boolean; }
export interface ScreenPolicy { enabled: boolean; max_width: number; max_height: number; max_fps: number; max_bitrate_kbps: number; allow_audio: boolean; max_viewers: number; max_rooms: number; }
export interface ScreenState { revision?: string; channel_id: string; share_id: string; host_id: string; settings: ScreenSettings; policy: ScreenPolicy; viewers: number; ready: boolean; paused?: boolean; }
export interface ScreenResponse extends ScreenState { request_id?: string; action: string; sdp?: string; error?: string; server_address: string; self_session_id: string; }
export interface ScreenRequest { request_id: string; action: string; channel_id: string; share_id?: string; settings?: ScreenSettings; sdp?: string; paused?: boolean; }
export const defaultScreenSettings: ScreenSettings = {width:1280,height:720,fps:30,bitrate_kbps:2000,audio:false};
export function fitScreenSettings(value: ScreenSettings, policy: ScreenPolicy): ScreenSettings {
  const bounded=(n:number,min:number,max:number,fallback:number)=>Math.max(min,Math.min(max,Number.isFinite(n)?Math.round(n):fallback));
  return {width:bounded(value.width,160,policy.max_width,1280),height:bounded(value.height,90,policy.max_height,720),fps:bounded(value.fps,1,policy.max_fps,30),bitrate_kbps:bounded(value.bitrate_kbps,128,policy.max_bitrate_kbps,2000),audio:value.audio&&policy.allow_audio};
}
export function screenShareError(error:unknown):string {
  const name=error&&typeof error==='object'&&'name' in error?String(error.name):'';
  switch(name){
    case 'NotAllowedError':return '未开始共享：已取消选择，或系统未授予屏幕录制权限。';
    case 'NotReadableError':return '系统暂时无法读取该画面，请检查屏幕录制权限，或选择其他窗口。';
    case 'NotFoundError':return '没有可共享的桌面或窗口。';
    case 'OverconstrainedError':return '当前共享源不支持所选画质，请降低分辨率或帧率后重试。';
    case 'InvalidStateError':return '请回到客户端，点击分享屏幕后重新选择画面。';
    default:return error instanceof Error?error.message:String(error);
  }
}
export type ScreenView = { room: ScreenState|null; stream: MediaStream|null; role: 'idle'|'publishing'|'viewing'; busy: boolean; error: string; notice: string; privacyPaused?: boolean; privacyBusy?: boolean };
export type ScreenContext = {server: string; session: string; channel: string};
type CapturedAudio = {track:MediaStreamTrack;stop:()=>void;pause?:(paused:boolean)=>Promise<void>};
type Pending = {resolve:(r:ScreenResponse)=>void;reject:(e:Error)=>void;timer:ReturnType<typeof setTimeout>};

// Owns all capture tracks and peer connections. Destroying the room view always
// stops local capture, even when signaling has already disconnected.
export class ScreenShareController {
  view: ScreenView={room:null,stream:null,role:'idle',busy:false,error:'',notice:''};
  private nativeAudio:CapturedAudio|null=null;
  private videoSender:RTCRtpSender|null=null;
  private privacy:{stream:MediaStream;stop:()=>void}|null=null;
  private sharedAudioEnabled=true;
  private pc: RTCPeerConnection|null=null;
  private cancelOffer:(()=>void)|null=null;
  private connectionTimer:ReturnType<typeof setTimeout>|undefined;
  private local: MediaStream|null=null;
  private pending=new Map<string,Pending>();
  private generation=0;
  private revision=0n;
  private disposed=false;
  private listener:(view:ScreenView)=>void=()=>{};
  constructor(private context:ScreenContext, private send:(request:ScreenRequest)=>Promise<void>, private captureAudio?: (onFailure:()=>void)=>Promise<CapturedAudio>){}
  onChange(callback:(view:ScreenView)=>void){this.listener=callback;callback(this.view);}
  private patch(patch:Partial<ScreenView>){this.view={...this.view,...patch};if(!this.disposed)this.listener(this.view);}
  receive(message:ScreenResponse){
    if(this.disposed||message.server_address!==this.context.server||message.self_session_id!==this.context.session||message.channel_id!==this.context.channel)return;
    if(message.request_id){const pending=this.pending.get(message.request_id);if(pending){clearTimeout(pending.timer);this.pending.delete(message.request_id);message.error?pending.reject(new Error(message.error)):pending.resolve(message);}}
    // Responses can race with newer broadcasts. Settle the RPC above, but
    // never let an older snapshot resurrect a stopped or replaced share.
    const revision=message.revision&&/^\d+$/.test(message.revision)?BigInt(message.revision):0n;
    if(revision<this.revision)return;
    this.revision=revision;
    if(message.action==='state'||message.action==='status'||message.action==='publish'||message.action==='subscribe'||message.action==='stop'||message.action==='cancel'||message.action==='leave'||message.action==='pause'){
      const previous=this.view.room?.share_id;
      this.patch({room:message});
      if(previous&&previous!==message.share_id){this.release();this.patch({error:message.error||'',notice:message.error?'':message.share_id?'房间共享已更换，点击观看加入新画面。':'共享已结束，语音通话继续。'});}
    }
  }
  private request(action:string,extra:Partial<ScreenRequest>={}):Promise<ScreenResponse>{
    if(this.disposed)return Promise.reject(new Error('共享界面已关闭'));
    const request_id=crypto.randomUUID();
    return new Promise((resolve,reject)=>{
      const timer=setTimeout(()=>{this.pending.delete(request_id);reject(new Error('屏幕共享请求超时，请重试'));},12000);
      this.pending.set(request_id,{resolve,reject,timer});
      void this.send({request_id,action,channel_id:this.context.channel,...extra}).catch(error=>{clearTimeout(timer);this.pending.delete(request_id);reject(error instanceof Error?error:new Error(String(error)));});
    });
  }
  async refresh(){try{await this.request('status');}catch(e){this.fail(e);}}
  private fail(e:unknown){if(!this.disposed)this.patch({error:screenShareError(e)});}
  private release(){this.generation++;clearTimeout(this.connectionTimer);this.connectionTimer=undefined;this.cancelOffer?.();this.nativeAudio?.stop();this.nativeAudio=null;this.privacy?.stop();this.privacy=null;this.videoSender=null;const pc=this.pc;this.pc=null;pc?.close();const local=this.local;this.local=null;local?.getTracks().forEach(t=>t.stop());this.patch({stream:null,role:'idle',busy:false,privacyPaused:false,privacyBusy:false});}
  async stop(){
    const share=this.view.room?.share_id;
    const role=this.view.role,busy=this.view.busy;this.release();
    try{await this.request(busy?'cancel':role==='viewing'?'leave':share?'stop':'cancel',{share_id:share});await this.refresh();}catch(e){this.fail(e);}
  }
  async closeByModerator(){try{await this.request('stop',{share_id:this.view.room?.share_id});await this.refresh();}catch(e){this.fail(e);}}
  private peer(generation:number){
    const pc=new RTCPeerConnection({iceServers:[],bundlePolicy:'max-bundle'});this.pc=pc;
    const current=()=>this.generation===generation&&!this.disposed&&this.pc===pc;
    const fail=()=>{if(!current())return;this.fail(new Error('共享连接中断，可重新观看或发起共享'));void this.stop();};
    const arm=(delay:number)=>{if(this.connectionTimer!==undefined)return;this.connectionTimer=setTimeout(()=>{this.connectionTimer=undefined;fail();},delay);};
    // Some runtimes linger in connecting/disconnected without reporting failed.
    // Bound capture lifetime, while giving brief network changes time to recover.
    arm(20000);
    pc.onconnectionstatechange=()=>{
      if(!current())return;
      if(pc.connectionState==='connected'){clearTimeout(this.connectionTimer);this.connectionTimer=undefined;}
      else if(pc.connectionState==='disconnected')arm(10000);
      else if(pc.connectionState==='failed'||pc.connectionState==='closed')fail();
    };
    return pc;
  }
  private async offer(pc:RTCPeerConnection){
    let change=()=>{};
    let timer:ReturnType<typeof setTimeout>|undefined;
    let cancel=()=>{};
    const complete=new Promise<void>((resolve,reject)=>{
      change=()=>{if(pc.iceGatheringState==='complete')resolve();};
      cancel=()=>reject(new Error('共享连接已取消'));
      timer=setTimeout(()=>reject(new Error('视频网络协商超时')),6000);
      pc.addEventListener('icegatheringstatechange',change);
    });
    this.cancelOffer=cancel;
    void complete.catch(()=>{});
    try{
      await pc.setLocalDescription(await pc.createOffer());
      change();
      await complete;
      return pc.localDescription!.sdp;
    }finally{
      clearTimeout(timer);
      pc.removeEventListener('icegatheringstatechange',change);
      if(this.cancelOffer===cancel)this.cancelOffer=null;
    }
  }
  private videoCodec(transceiver:RTCRtpTransceiver){
    const codecs=RTCRtpSender.getCapabilities?.('video')?.codecs.filter(c=>c.mimeType.toLowerCase()==='video/vp8');
    if(codecs&&!codecs.length)throw new Error('当前客户端不支持所需的视频编码');
    // Older WebView2 runtimes may lack preferences. The SFU only offers VP8,
    // so standard SDP negotiation still chooses the supported codec.
    if(codecs&&typeof transceiver.setCodecPreferences==='function')transceiver.setCodecPreferences(codecs);
  }
  async publish(settings:ScreenSettings,source:'monitor'|'window'|'system'){
    if(this.view.busy||this.view.role!=='idle')return;
    if(!this.view.room?.policy.enabled){this.fail(new Error('服务器尚未启用屏幕共享'));return;}
    if(!navigator.mediaDevices?.getDisplayMedia){this.fail(new Error('当前系统内嵌浏览器不支持屏幕采集'));return;}
    const actual=fitScreenSettings(settings,this.view.room.policy);
    const generation=++this.generation;this.sharedAudioEnabled=true;this.patch({busy:true,error:'',notice:'',privacyPaused:false,privacyBusy:false});
    let captured:MediaStream|null=null;
    try{
      // Must stay in the user gesture: no signaling await before the picker.
      captured=await navigator.mediaDevices.getDisplayMedia({video:{...(source==='system'?{}:{displaySurface:source}),frameRate:{ideal:actual.fps,max:actual.fps},width:{ideal:actual.width,max:actual.width},height:{ideal:actual.height,max:actual.height}},audio:actual.audio&&!this.captureAudio,systemAudio:actual.audio&&!this.captureAudio?'include':'exclude',selfBrowserSurface:'exclude'} as DisplayMediaStreamOptions);
      if(this.disposed||generation!==this.generation){captured.getTracks().forEach(t=>t.stop());return;}
      this.local=captured;
      const video=captured.getVideoTracks()[0];if(!video)throw new Error('没有获得共享画面');
      // The user can stop capture in the OS picker while constraints are pending.
      video.onended=()=>{if(this.generation===generation)void this.stop();};
      const requireLiveVideo=()=>{if(video.readyState==='ended')throw new Error('共享画面已停止，请重新选择');};
      requireLiveVideo();
      await video.applyConstraints({width:{max:actual.width},height:{max:actual.height},frameRate:{max:actual.fps}});
      if(this.disposed||generation!==this.generation){captured.getTracks().forEach(t=>t.stop());return;}
      requireLiveVideo();
      video.contentHint='detail';
      if(!actual.audio)captured.getAudioTracks().forEach(t=>{captured!.removeTrack(t);t.stop();});
      if(actual.audio&&!captured.getAudioTracks().length&&this.captureAudio){
        const audio=await this.captureAudio(()=>{if(generation===this.generation){this.fail(new Error('系统声音采集已中断，请重新开始共享'));void this.stop();}});
        if(this.disposed||generation!==this.generation){audio.stop();return;}
        this.nativeAudio=audio;captured.addTrack(audio.track);
      }
      if(actual.audio&&!captured.getAudioTracks().length){actual.audio=false;this.patch({notice:'系统没有提供共享音轨，本次仅共享画面。'});}
      const pc=this.peer(generation);
      const transceiver=pc.addTransceiver(video,{direction:'sendonly',streams:[captured],sendEncodings:[{maxBitrate:actual.bitrate_kbps*1000,maxFramerate:actual.fps}]});this.videoCodec(transceiver);this.videoSender=transceiver.sender;
      for(const track of captured.getAudioTracks()){pc.addTransceiver(track,{direction:'sendonly',streams:[captured],sendEncodings:[{maxBitrate:128000}]});}
      const sdp=await this.offer(pc);
      if(generation!==this.generation||this.disposed)return;
      const result=await this.request('publish',{settings:actual,sdp});
      if(generation!==this.generation||this.disposed)return;
      if(this.view.room?.share_id!==result.share_id){this.release();return;}
      await pc.setRemoteDescription({type:'answer',sdp:result.sdp});
      if(generation!==this.generation||this.disposed)return;
      this.patch({role:'publishing',stream:captured,busy:false});
    }catch(e){if(generation===this.generation){this.release();this.fail(e);void this.request('cancel').catch(()=>{});}else captured?.getTracks().forEach(t=>t.stop());}
  }
  async watch(){
    const room=this.view.room;if(this.view.busy||this.view.role!=='idle'||!room?.ready)return;
    const generation=++this.generation;this.patch({busy:true,error:'',notice:''});
    try{
      const pc=this.peer(generation),stream=new MediaStream();
      pc.ontrack=e=>{if(generation===this.generation){stream.addTrack(e.track);this.patch({stream});}};
      const video=pc.addTransceiver('video',{direction:'recvonly'});this.videoCodec(video);
      if(room.settings.audio)pc.addTransceiver('audio',{direction:'recvonly'});
      const sdp=await this.offer(pc);if(generation!==this.generation)return;
      const result=await this.request('subscribe',{share_id:room.share_id,sdp});if(generation!==this.generation)return;
      if(this.view.room?.share_id!==room.share_id||result.share_id!==room.share_id){this.release();return;}
      await pc.setRemoteDescription({type:'answer',sdp:result.sdp});if(generation!==this.generation||this.disposed)return;this.patch({stream,role:'viewing',busy:false});
    }catch(e){if(generation===this.generation){this.release();this.fail(e);void this.request('leave',{share_id:room.share_id}).catch(()=>{});}}
  }
  private privacyStream() {
    const canvas=document.createElement('canvas');
    const size=this.local?.getVideoTracks()[0]?.getSettings?.();
    canvas.width=Math.max(160,Math.min(1280,size?.width||1280));
    canvas.height=Math.max(90,Math.round(canvas.width*(size?.height||720)/(size?.width||1280)));
    const context=canvas.getContext('2d');
    if(!context||!canvas.captureStream)throw new Error('当前客户端无法创建隐私画面');
    const draw=()=>{context.fillStyle='#171a25';context.fillRect(0,0,canvas.width,canvas.height);context.textAlign='center';context.fillStyle='#b8adcf';context.font=`${Math.max(14,canvas.width/42)}px system-ui`;context.fillText('SCREEN SHARING PAUSED',canvas.width/2,canvas.height*.44);context.fillStyle='#f2edf8';context.font=`${Math.max(18,canvas.width/26)}px system-ui`;context.fillText('共享已暂停',canvas.width/2,canvas.height*.55);};
    draw();const stream=canvas.captureStream(2),timer=setInterval(draw,500);
    return {stream,stop:()=>{clearInterval(timer);stream.getTracks().forEach(track=>track.stop());}};
  }
  async setPrivacyPaused(paused:boolean) {
    if(this.view.role!=='publishing'||this.view.privacyBusy||paused===!!this.view.privacyPaused)return;
    if(paused&&typeof this.view.room?.paused!=='boolean'){this.fail(new Error('服务器尚未支持隐私暂停'));return;}
    const generation=this.generation,share=this.view.room?.share_id,source=this.local?.getVideoTracks()[0],sender=this.videoSender;
    if(!source||!sender||!share)return;
    const current=()=>generation===this.generation&&!this.disposed&&this.view.role==='publishing';
    this.patch({privacyBusy:true,error:'',notice:''});
    if(paused){
      // Fail closed before any asynchronous work: real video and shared audio
      // immediately become zero-information tracks, including native WebView capture.
      source.enabled=false;this.local?.getAudioTracks().forEach(track=>{track.enabled=false;});
      this.patch({privacyPaused:true});
      let protectedMedia=false;
      try{
        this.privacy=this.privacyStream();
        await sender.replaceTrack(this.privacy.stream.getVideoTracks()[0]);
        if(!current())return;
        this.patch({stream:this.privacy.stream});
        await this.nativeAudio?.pause?.(true);
        if(!current())return;
        protectedMedia=true;
        const result=await this.request('pause',{share_id:share,paused:true});
        if(!current())return;
        if(result.paused!==true)throw new Error('服务器尚未确认隐私暂停');
        this.patch({notice:'隐私暂停中：画面与共享声音已隐藏，语音通话继续。'});
      }catch(error){
        if(!current())return;
        if(!protectedMedia){await this.stop();this.fail(new Error('隐私保护未能完成，共享已停止。'+screenShareError(error)));}
        else this.fail(new Error('画面与共享声音已隐藏，暂停状态同步失败：'+screenShareError(error)));
      }finally{if(current())this.patch({privacyBusy:false});}
      return;
    }
    try{
      if(source.readyState==='ended')throw new Error('原共享源已结束，请结束共享后重新选择');
      const result=await this.request('pause',{share_id:share,paused:false});
      if(!current())return;
      if(result.paused!==false)throw new Error('服务器尚未确认恢复');
      await this.nativeAudio?.pause?.(false);
      if(!current())return;
      await sender.replaceTrack(source);
      if(!current())return;
      source.enabled=true;this.local?.getAudioTracks().forEach(track=>{track.enabled=this.sharedAudioEnabled;});
      this.privacy?.stop();this.privacy=null;
      this.patch({stream:this.local,privacyPaused:false,privacyBusy:false,notice:'已恢复共享。'});
    }catch(error){
      if(!current())return;
      source.enabled=false;this.local?.getAudioTracks().forEach(track=>{track.enabled=false;});
      try{await this.nativeAudio?.pause?.(true);}catch{this.nativeAudio?.stop();}
      if(!current())return;
      void this.request('pause',{share_id:share,paused:true}).catch(()=>{});
      this.patch({privacyBusy:false,error:'未能恢复，共享仍处于隐私暂停：'+screenShareError(error)});
    }
  }
  setSharedAudio(enabled:boolean){
    this.sharedAudioEnabled=enabled;
    if(this.view.role==='publishing')this.local?.getAudioTracks().forEach(t=>{t.enabled=enabled&&!this.view.privacyPaused;});
  }
  dispose(){
    if(this.disposed)return;
    const action=this.view.role==='viewing'?'leave':'cancel';
    void this.send({request_id:crypto.randomUUID(),channel_id:this.context.channel,share_id:this.view.room?.share_id,action}).catch(()=>{});
    this.disposed=true;this.release();
    for(const p of this.pending.values()){clearTimeout(p.timer);p.reject(new Error('共享界面已关闭'));}this.pending.clear();
  }
}
