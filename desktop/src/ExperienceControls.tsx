import {useEffect,useRef,useState} from 'react';
import {Search,X,BellOff} from 'lucide-react';
import {dispatch} from './bridge';
import {preferences,roomKey,roomPreferences,updatePreferences,updateRoom,diagnosticAdvice} from './experienceState';
import type {CoreSnapshot} from './types';

export function NotificationControls({snapshot,channel}:{snapshot:CoreSnapshot;channel:string}){
  const [,render]=useState(0);
  const root=useRef<HTMLDetailsElement>(null);
  useEffect(()=>{
    const outside=(event:PointerEvent)=>{if(root.current&&!root.current.contains(event.target as Node))root.current.open=false;};
    const escape=(event:KeyboardEvent)=>{if(event.key==='Escape'&&root.current?.open){root.current.open=false;root.current.querySelector('summary')?.focus();}};
    window.addEventListener('pointerdown',outside);window.addEventListener('keydown',escape);
    return()=>{window.removeEventListener('pointerdown',outside);window.removeEventListener('keydown',escape);};
  },[]);
  useEffect(()=>{if(root.current)root.current.open=false;},[channel]);
  useEffect(()=>{const changed=()=>render(x=>x+1);window.addEventListener('eden-experience-changed',changed);return()=>window.removeEventListener('eden-experience-changed',changed);},[]);
  const prefs=preferences(),key=roomKey(snapshot,channel),muted=roomPreferences(key)?.muted||false;
  return <details ref={root} className="notification-controls"><summary aria-label="通知设置"><BellOff size={14}/><span>{prefs.dnd?'免打扰':'通知'}</span></summary><section aria-label="通知偏好"><label><input type="checkbox" checked={prefs.dnd} onChange={e=>updatePreferences({dnd:e.target.checked})}/>免打扰（保留未读标记）</label><label><input type="checkbox" checked={prefs.sounds} onChange={e=>updatePreferences({sounds:e.target.checked})}/>播放进出房间及消息提示音</label>{channel&&<label><input type="checkbox" checked={muted} onChange={e=>updateRoom(key,{muted:e.target.checked})}/>静音当前房间通知</label>}</section></details>;
}

export function Advice({snapshot,onAction}:{snapshot:CoreSnapshot;onAction?:(action:'audio'|'network'|'diagnostics')=>void}){
  return <div className="voice-advice">{diagnosticAdvice(snapshot).map(tip=><article key={tip.title}><strong>{tip.title}</strong><p>{tip.detail}</p>{onAction&&<button onClick={()=>onAction(tip.action)}>{tip.action==='audio'?'打开音频设置':tip.action==='network'?'查看网络详情':'查看诊断'}</button>}</article>)}</div>;
}

export function DeviceNotice({snapshot}:{snapshot:CoreSnapshot}){
  const [dismissed,setDismissed]=useState('');
  const [changed,setChanged]=useState<{key:string;input:string;output:string}|null>(null);
  const previous=useRef<{scope:string;input:string;output:string}|null>(null);
  const {audio:a}=snapshot;
  const scope=JSON.stringify([snapshot.serverAddress,snapshot.announcementScope,snapshot.selfSessionId]);
  useEffect(()=>{
    if(snapshot.connection!=='connected'){previous.current=null;setChanged(null);return;}
    const old=previous.current;
    if(old?.scope===scope&&old.input&&old.output&&a.activeInput&&a.activeOutput&&(old.input!==a.activeInput||old.output!==a.activeOutput))setChanged({key:JSON.stringify([scope,a.activeInput,a.activeOutput]),input:a.activeInput,output:a.activeOutput});
    previous.current={scope,input:a.activeInput,output:a.activeOutput};
  },[scope,snapshot.connection,a.activeInput,a.activeOutput]);
  const inputFallback=!!a.selectedInput&&a.selectedInput!=='系统默认麦克风'&&!!a.activeInput&&a.selectedInput!==a.activeInput;
  const outputFallback=!!a.selectedOutput&&a.selectedOutput!=='系统默认输出'&&!!a.activeOutput&&a.selectedOutput!==a.activeOutput;
  const key=JSON.stringify([inputFallback,outputFallback,a.activeInput,a.activeOutput,a.selectedInput,a.selectedOutput]);
  useEffect(()=>{
    if(!inputFallback&&!outputFallback)return;
    const timer=setInterval(()=>void dispatch({type:'refresh_devices'}).catch(()=>{}),3000);
    return()=>clearInterval(timer);
  },[key]);
  if(snapshot.connection!=='connected')return null;
  if(!inputFallback&&!outputFallback){
    if(!changed||dismissed===changed.key)return null;
    return <aside className="device-notice" role="status"><div><strong>音频设备已切换</strong><p>麦克风：{changed.input} · 扬声器：{changed.output}</p></div><button onClick={()=>window.dispatchEvent(new Event('eden-open-audio-settings'))}>音频设置</button><button aria-label="关闭设备提示" onClick={()=>setDismissed(changed.key)}><X size={14}/></button></aside>;
  }
  if(dismissed===key)return null;
  const restored=(!inputFallback||a.inputs.includes(a.selectedInput))&&(!outputFallback||a.outputs.includes(a.selectedOutput));
  return <aside className="device-notice" role="status"><div><strong>正在使用备用音频设备</strong><p>{inputFallback&&`麦克风：${a.activeInput}。`}{outputFallback&&`扬声器：${a.activeOutput}。`}{restored?'原设备已可用，可以切回。':'原设备暂不可用，连接恢复后可切回。'}</p></div><button disabled={!restored} onClick={()=>void dispatch({type:'retry_audio'}).catch(()=>{})}>恢复原设备</button><button aria-label="关闭设备提示" onClick={()=>setDismissed(key)}><X size={14}/></button></aside>;
}

export function QuickJump({snapshot,close,onMember}:{snapshot:CoreSnapshot;close:()=>void;onMember:(id:string)=>void}){
  const [query,setQuery]=useState(''),[selected,setSelected]=useState(0),[busy,setBusy]=useState(false),[error,setError]=useState('');
  const input=useRef<HTMLInputElement>(null),previous=useRef(document.activeElement as HTMLElement|null);
  useEffect(()=>{input.current?.focus();return()=>previous.current?.focus();},[]);
  const needle=query.trim().toLocaleLowerCase();
  const items=[...snapshot.channels.map(channel=>({key:'r'+channel.id,label:channel.name,kind:'房间',action:async()=>{await dispatch({type:'join_channel',channelId:channel.id});close();}})),...snapshot.users.map(user=>({key:'u'+user.sessionId,label:user.nickname,kind:'成员',action:async()=>{onMember(user.sessionId);close();}})),...snapshot.savedServers.map((server,index)=>({key:'s'+index,label:server.displayName||server.serverName||server.address,kind:'服务器',action:async()=>{window.dispatchEvent(new CustomEvent('eden-open-saved-server',{detail:server}));close();}}))].filter(item=>`${item.kind} ${item.label}`.toLocaleLowerCase().includes(needle)).slice(0,30);
  const run=async(index:number)=>{if(busy||!items[index])return;setBusy(true);try{await items[index].action();}catch(e){setError(String(e));}finally{setBusy(false);}};
  return <div className="modal-backdrop" onMouseDown={close}><section className="quick-jump" role="dialog" aria-modal="true" aria-label="快捷跳转" onMouseDown={e=>e.stopPropagation()} onKeyDown={e=>{if(e.key==='Escape'){e.stopPropagation();close();}if(e.key==='ArrowDown'||e.key==='ArrowUp'){e.preventDefault();setSelected(i=>items.length?(i+(e.key==='ArrowDown'?1:-1)+items.length)%items.length:0);}if(e.key==='Enter'){e.preventDefault();void run(Math.min(selected,items.length-1));}if(e.key==='Tab'){e.preventDefault();input.current?.focus();}}}><header><Search size={17}/><input ref={input} role="combobox" aria-expanded="true" aria-autocomplete="list" value={query} onChange={e=>{setQuery(e.target.value);setSelected(0);}} placeholder="搜索服务器、房间或成员" aria-label="搜索跳转目标" aria-controls="quick-jump-results" aria-activedescendant={items.length?`jump-${items[Math.min(selected,items.length-1)].key}`:undefined}/><button aria-label="关闭快捷跳转" onClick={close}><X size={16}/></button></header><div id="quick-jump-results" role="listbox">{items.map((item,i)=><button type="button" role="option" aria-selected={i===selected} disabled={busy} id={`jump-${item.key}`} key={item.key} onClick={()=>void run(i)}><span>{item.label}</span><small>{item.kind}</small></button>)}{!items.length&&<p>没有匹配的结果</p>}</div>{error&&<p role="alert">{error}</p>}<footer>↑ ↓ 选择 · Enter 跳转 · Esc 关闭</footer></section></div>;
}
