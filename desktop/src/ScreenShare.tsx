import { useEffect, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { MonitorUp, Play, Square, X, Volume2, VolumeX, Maximize2, Monitor, AppWindow, Users, Radio, ShieldCheck, Pause } from 'lucide-react';
import {startNativeScreenAudio} from './nativeScreenAudio';
import { dispatch } from './bridge';
import type { CoreSnapshot } from './types';
import { ScreenShareController, defaultScreenSettings, fitScreenSettings, type ScreenView, type ScreenSettings, type ScreenResponse } from './screenShareController';

export function ScreenShare({snapshot,channelId,selected,onSelect,onBack}:{snapshot:CoreSnapshot;channelId:string;selected?:boolean;onSelect?:()=>void;onBack?:()=>void}){
  const [controller,setController]=useState<ScreenShareController|null>(null);
  const [view,setView]=useState<ScreenView>({room:null,stream:null,role:'idle',busy:false,error:'',notice:''});
  const [open,setOpen]=useState(false),[localExpanded,setLocalExpanded]=useState(true);
  const expanded=selected??localExpanded;
  const setExpanded=(value:boolean)=>{setLocalExpanded(value);if(value)onSelect?.();else onBack?.();};
  const [settings,setSettings]=useState<ScreenSettings>(defaultScreenSettings);
  const [source,setSource]=useState<'monitor'|'window'>('monitor');
  const [sharedAudio,setSharedAudio]=useState(true),[muted,setMuted]=useState(false),[needsPlay,setNeedsPlay]=useState(false);
  const video=useRef<HTMLVideoElement>(null),videoFrame=useRef<HTMLDivElement>(null);
  const isMac=/Mac/i.test(navigator.platform);
  const supported=!!snapshot.screenShareSupported&&snapshot.connection==='connected'&&!!channelId;
  useEffect(()=>{
    if(!supported)return;
    let disposed=false,unlisten:(()=>void)|undefined;
    const c=new ScreenShareController({server:snapshot.serverAddress,session:snapshot.selfSessionId,channel:channelId},request=>dispatch({type:'screen_share',server_address:snapshot.serverAddress,self_session_id:snapshot.selfSessionId,request}),/Mac/i.test(navigator.platform)&&'__TAURI_INTERNALS__' in window?failure=>startNativeScreenAudio({server:snapshot.serverAddress,session:snapshot.selfSessionId,channel:channelId},failure):undefined);
    c.onChange(setView);setController(c);
    void listen<ScreenResponse>('screen-share-state',event=>c.receive(event.payload)).then(stop=>{if(disposed){stop();return;}unlisten=stop;void c.refresh();}).catch(e=>setView(v=>({...v,error:String(e)})));
    return()=>{disposed=true;unlisten?.();c.dispose();setController(null);};
  },[supported,snapshot.serverAddress,snapshot.selfSessionId,channelId]);
  useEffect(()=>{
    const element=video.current;if(!element)return;
    element.srcObject=view.stream;
    if(view.stream)void element.play().then(()=>setNeedsPlay(false)).catch(()=>setNeedsPlay(true));
    return()=>{element.srcObject=null;};
  },[view.stream]);
  if(!supported)return null;
  const room=view.room,policy=room?.policy;
  const host=snapshot.users.find(u=>u.sessionId===room?.host_id)?.nickname||'房间成员';
  const canModerate=snapshot.capabilities.includes('watch.manage');
  const active=view.role!=='idle';
  const paused=view.role==='publishing'?!!view.privacyPaused:!!room?.paused;
  const apply=(value:ScreenSettings)=>setSettings(policy?fitScreenSettings(value,policy):value);
  const begin=()=>{if(!controller||!policy)return;setOpen(false);setExpanded(true);setSharedAudio(true);void controller.publish(fitScreenSettings(settings,policy),isMac?'system':source);};
  return <div className="screen-share">
    <div className="screen-strip"><div className={`screen-share-bar ${room?.share_id?'is-live':''}`}><span className="screen-share-symbol"><MonitorUp size={17}/></span><div className="screen-share-copy"><strong>{view.busy?'正在连接共享…':room?.share_id?paused?`${host} 的共享已暂停`:`${host} 正在共享屏幕`:'一起看看你的屏幕'}</strong><small>{paused?'隐私暂停 · 语音通话继续':active?(expanded?'正在观看 · 语音通话继续':'后台连接中 · 共享声音继续'):room?.share_id?'仅同房间成员可观看 · 点击加入':policy?.enabled?'分享游戏、窗口，或整个桌面':'服务器尚未开启屏幕共享'}</small></div>
      {room?.share_id&&<small className="screen-audience"><Users size={12}/>{room.viewers} 人观看</small>}
      <div className="screen-share-actions">
        {view.role==='publishing'&&(typeof room?.paused==='boolean'||view.privacyPaused)&&<button className={`screen-privacy-toggle ${paused?'is-paused':''}`} disabled={view.privacyBusy} aria-pressed={paused} onClick={()=>void controller?.setPrivacyPaused(!paused)}>{paused?<Play size={13}/>:<Pause size={13}/>} {view.privacyBusy?'正在处理…':paused?'恢复共享':'隐私暂停'}</button>}
        {active&&<button onClick={()=>setExpanded(!expanded)}>{expanded?'返回房间':'查看画面'}</button>}
        {view.busy?<button onClick={()=>void controller?.stop()}>取消连接</button>:active?<button className="screen-stop" onClick={()=>void controller?.stop()}><Square size={12}/>{view.role==='publishing'?'结束共享':'停止观看'}</button>:room?.share_id?<><button className="primary" disabled={!room.ready} onClick={()=>{setExpanded(true);void controller?.watch();}}><Play size={12}/>{room.ready?'观看':'正在准备'}</button>{canModerate&&<button onClick={()=>void controller?.closeByModerator()}>结束共享</button>}</>:<button disabled={!policy?.enabled} title={!policy?.enabled?'服务器未开启屏幕共享':''} onClick={()=>{if(policy)apply(settings);setOpen(true);}}>分享屏幕</button>}
      {canModerate&&view.role==='viewing'&&!view.busy&&<button className="screen-stop" onClick={()=>void controller?.closeByModerator()}>结束共享</button>}</div>
    </div>
    {(view.error||view.notice)&&<div className="screen-share-notice" role={view.error?'alert':'status'}>{view.error||view.notice}</div>}</div>
    {active&&<div className="screen-viewer" hidden={!expanded}>
      <header><span className="screen-viewer-title"><span className={`screen-live-label ${paused?'is-paused':''}`}>{paused?<><ShieldCheck size={12}/> 已暂停</>:<><Radio size={12}/> LIVE</>}</span><span>{view.role==='publishing'?'你的共享预览':`${host} 的屏幕`}<small>画质上限 {room?.settings.width}×{room?.settings.height} · {room?.settings.fps} fps</small></span></span></header>
      <div ref={videoFrame} className={`screen-video ${paused?'is-paused':''}`}><video ref={video} autoPlay playsInline muted={paused||view.role==='publishing'||muted} aria-hidden={paused}/>{paused&&<div className="screen-privacy-placeholder" role="status"><span><ShieldCheck size={30} strokeWidth={1.5}/></span><small>PRIVACY PAUSE</small><h3>共享已暂停</h3><p>{view.role==='publishing'?'画面与共享声音已隐藏':'分享者暂时离开，回来后会继续共享'}</p><em>语音通话继续</em></div>}{!paused&&needsPlay&&<button className="screen-play" onClick={()=>void video.current?.play().then(()=>setNeedsPlay(false)).catch(()=>setNeedsPlay(true))}>点击播放画面与声音</button>}</div>
      <footer><small>{paused?'共享声音已暂停 · 语音通话继续':'切换标签不退出观看'}</small><div>{view.role==='publishing'&&room?.settings.audio?<button disabled={paused} onClick={()=>{controller?.setSharedAudio(!sharedAudio);setSharedAudio(!sharedAudio);}}>{sharedAudio&&!paused?<Volume2 size={15}/>:<VolumeX size={15}/>}共享声音{paused?'已暂停':sharedAudio?'已开':'已关'}</button>:view.role==='viewing'&&room?.settings.audio?<button disabled={paused} onClick={()=>setMuted(v=>!v)}>{muted||paused?<VolumeX size={15}/>:<Volume2 size={15}/>}共享声音</button>:null}<button onClick={()=>void videoFrame.current?.requestFullscreen().catch(()=>{})}><Maximize2 size={15}/>全屏</button></div></footer>
    </div>}
    {!active&&expanded&&<div className="screen-empty"><MonitorUp size={36}/><h3>{view.busy?'正在连接画面…':room?.share_id?`${host} 的屏幕`:'一起分享眼前的精彩'}</h3><p>{view.busy?'连接完成后，画面会显示在这里。':room?.share_id?(paused?'分享者暂时离开。你可以加入观看，等待共享恢复。':'点击上方“观看”加入，仅当前房间成员可见。'):'点击上方“分享屏幕”，选择想给朋友看的桌面或窗口。'}</p></div>}
    {open&&policy&&<div className="screen-dialog-backdrop" onClick={()=>setOpen(false)}><section className="screen-dialog" role="dialog" aria-modal="true" aria-label="分享屏幕" onClick={e=>e.stopPropagation()}>
      <header><div><span className="screen-dialog-kicker">SCREEN SHARE · 同房间可见</span><h3>让朋友看看你的屏幕</h3><p>选择画面，保持通话。</p></div><button aria-label="关闭共享设置" onClick={()=>setOpen(false)}><X size={17}/></button></header>
      {!isMac&&<div className="screen-source"><button aria-label="整个桌面" aria-pressed={source==='monitor'} onClick={()=>setSource('monitor')}><Monitor size={22}/><span>整个桌面<small>展示桌面上的所有内容</small></span></button><button aria-label="程序窗口" aria-pressed={source==='window'} onClick={()=>setSource('window')}><AppWindow size={22}/><span>程序窗口<small>专注一个应用的画面</small></span></button></div>}
      <p className="screen-help">{isMac?'在系统选择器中点选窗口；共享桌面请选择右上角的“共享整个屏幕”。':'点击开始后，在系统选择器中确认要共享的桌面或窗口。'}</p>
      <div className="screen-settings-grid"><label>分辨率<select value={`${settings.width}x${settings.height}`} onChange={e=>{const [width,height]=e.target.value.split('x').map(Number);apply({...settings,width,height});}}>{Array.from(new Map([[640,360],[1280,720],[1920,1080],[2560,1440],[3840,2160],[settings.width,settings.height]].filter(([w,h])=>w<=policy.max_width&&h<=policy.max_height).map(([w,h])=>[`${w}x${h}`,[w,h]])).values()).map(([w,h])=><option key={`${w}x${h}`} value={`${w}x${h}`}>{w} × {h}</option>)}</select></label>
      <label>帧率<input type="number" min={1} max={policy.max_fps} value={settings.fps} onChange={e=>apply({...settings,fps:Math.round(Number(e.target.value)||1)})}/></label>
      <label>码率（kbps）<input type="number" min={128} max={policy.max_bitrate_kbps} step={100} value={settings.bitrate_kbps} onChange={e=>apply({...settings,bitrate_kbps:Math.round(Number(e.target.value)||128)})}/></label></div>
      <label className="screen-audio-switch"><span>同时共享声音<small>{policy.allow_audio?(isMac?'共享系统播放声音，不采集麦克风；需允许系统音频录制。':'声音是否可共享取决于系统选择器；共享系统声音可能包含通话声，建议保持关闭以避免回声。'):'服务器已禁止共享声音'}</small></span><input type="checkbox" checked={settings.audio} disabled={!policy.allow_audio} onChange={e=>apply({...settings,audio:e.target.checked})}/></label>
      <p className="screen-help">服务器上限：{policy.max_width}×{policy.max_height} · {policy.max_fps} fps · {policy.max_bitrate_kbps} kbps。实际画质会受网络和电脑性能影响。</p>
      <footer><button onClick={()=>setOpen(false)}>取消</button><button className="primary" onClick={begin}>{isMac?'选择屏幕或窗口':'选择画面并开始'}</button></footer>
    </section></div>}
  </div>;
}
