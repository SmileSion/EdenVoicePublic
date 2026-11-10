import {useEffect,useRef,useState} from 'react';
import {Download,X} from 'lucide-react';
import {invoke} from '@tauri-apps/api/core';
import {listen} from '@tauri-apps/api/event';
type Release={version:string;url:string;downloadUrl:string|null;automatic?:boolean;notes?:string};
type Pending={version:string};
export function newerVersion(candidate:string,current='0.8.5'){const parse=(v:string)=>/^v?\d+\.\d+\.\d+$/.test(v)?v.replace(/^v/,'').split('.').map(Number):null;const a=parse(candidate),b=parse(current);if(!a||!b)return false;for(let i=0;i<3;i++){if(a[i]!==b[i])return a[i]>b[i];}return false;}
export function UpdateNotice({settings=false}:{settings?:boolean}){
 const [release,setRelease]=useState<Release|null>(null),[pending,setPending]=useState<Pending|null>(null),[busy,setBusy]=useState(false),[message,setMessage]=useState(''),[dismissed,setDismissed]=useState(false),[dialog,setDialog]=useState(false),[inCall,setInCall]=useState(false),[progress,setProgress]=useState('');
 const busyRef=useRef(false);busyRef.current=busy;
 const dialogRef=useRef<HTMLElement>(null);
 useEffect(()=>{if(!dialog)return;const previous=document.activeElement as HTMLElement|null;dialogRef.current?.querySelector<HTMLButtonElement>('button')?.focus();return()=>previous?.focus();},[dialog]);
 const refresh=()=>invoke<Pending|null>('pending_update').then(setPending).catch(()=>{});
 const check=async(manual:boolean)=>{if(busyRef.current)return;if(!('__TAURI_INTERNALS__' in window)){if(manual)setMessage('请在桌面客户端中检查更新');return;}setBusy(true);setMessage('');try{const next=await invoke<Release|null>('check_update');try{localStorage.setItem('eden.update.checked',String(Date.now()));}catch{}if(next&&newerVersion(next.version)){setRelease(previous=>{if(previous?.version!==next.version)setDismissed(false);return next;});}else{setRelease(null);if(manual)setMessage('当前已是最新正式版本');}}catch{/* Unreachable update services stay silent. */}finally{setBusy(false);}};
 useEffect(()=>{
  if(!('__TAURI_INTERNALS__' in window))return;
  void refresh();
  const changed=()=>{setProgress('');void refresh();};window.addEventListener('eden-update-changed',changed);
  const listener=listen<{downloaded:number;total?:number}>('eden-update-progress',({payload:p})=>setProgress(p.total?`${Math.min(100,Math.round(p.downloaded/p.total*100))}%`:`${(p.downloaded/1048576).toFixed(1)} MB`));
  return()=>{window.removeEventListener('eden-update-changed',changed);void listener.then(fn=>fn()).catch(()=>{});};
 },[]);
 useEffect(()=>{if(settings)return;const first=setTimeout(()=>void check(false),15000);const poll=setInterval(()=>{if(!document.hidden)void check(false);},60000);return()=>{clearTimeout(first);clearInterval(poll);};},[settings]);

 const changed=()=>{void refresh();window.dispatchEvent(new Event('eden-update-changed'));};
 const install=async(allowDisconnect=false)=>{
  setBusy(true);setMessage('');try{await invoke('install_update',{allowDisconnect});}catch(error){if(String(error).includes('IN_CALL')){setInCall(true);setDialog(true);}else{setMessage('更新已保留，将在下次启动时重试');setDialog(false);}}finally{setBusy(false);}
 };
 const download=async(now:boolean)=>{
  if(!release)return;setBusy(true);setMessage('');setProgress('准备下载…');
  try{await invoke('download_update',{version:release.version});changed();setProgress('');setDialog(false);if(now)await install();else setMessage('已下载，下次启动时安装');}
  catch{/* Network failures never interrupt calls with error prompts. */setDialog(false);}
  finally{setBusy(false);setProgress('');}
 };
 const cancel=async()=>{setBusy(true);try{await invoke('cancel_pending_update');changed();setMessage('已取消下次启动更新');}catch{}finally{setBusy(false);}};

 const action=pending?<><button disabled={busy} onClick={()=>{setInCall(false);setDialog(true);}}>安装并重启</button><button disabled={busy} onClick={()=>void cancel()}>取消待更新</button></>:release?<button disabled={busy} onClick={()=>setDialog(true)}>更新选项</button>:null;
 const modal=dialog&&<div className="update-dialog-backdrop"><section ref={dialogRef} className="update-dialog" onKeyDown={event=>{if(event.key==='Escape'&&!busy){event.stopPropagation();setDialog(false);}if(event.key==='Tab'){const buttons=Array.from(dialogRef.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)')||[]);const first=buttons[0],last=buttons[buttons.length-1];if(!first){event.preventDefault();return;}if(event.shiftKey&&document.activeElement===first){event.preventDefault();last.focus();}else if(!event.shiftKey&&document.activeElement===last){event.preventDefault();first.focus();}}}} role="dialog" aria-modal="true" aria-label="更新客户端">
  <strong>{inCall?'安装将结束当前通话':`更新至 ${pending?.version||release?.version}`}</strong>
  <p>{inCall?'你正在语音房间中。现在安装会退出通话并重启客户端，也可以留到下次启动。':pending?'安装完成后客户端会重新启动。':'下载并验证安装包后更新。你可以现在安装，也可以留到下次启动，当前通话不受下载影响。'}</p>
  {release?.notes&&<p style={{whiteSpace:'pre-wrap',maxHeight:120,overflowY:'auto'}}>{release.notes}</p>}{progress&&<p role="status">正在下载 {progress}</p>}
  <div className="update-dialog-actions">{pending?<><button disabled={busy} onClick={()=>void install(inCall)}>{inCall?'结束通话并安装':'确认安装并重启'}</button><button disabled={busy} onClick={()=>{setDialog(false);setInCall(false);}}>下次启动安装</button></>:<><button disabled={busy} onClick={()=>void download(true)}>下载并安装</button><button disabled={busy} onClick={()=>void download(false)}>下载后下次启动安装</button></>}
  <button disabled={busy} onClick={()=>setDialog(false)}>稍后</button></div>
 </section></div>;
 if(settings)return <><section className="update-settings"><strong>客户端更新</strong><p>{progress?`正在下载 ${progress}`:message|| (pending?`${pending.version} 已就绪，下次启动时安装`:release?`发现 ${release.version}`:'发现新版本后由你确认下载与安装时机。')}</p><button disabled={busy} onClick={()=>void check(true)}>{busy?'处理中…':'检查更新'}</button>{action}</section>{modal}</>;
 if((!release&&!pending)||dismissed)return modal;
 return <><aside className="update-notice" role="status"><Download size={15}/><div><strong>{pending?'待安装':'新版本'} {pending?.version||release?.version}</strong><small>{progress?`正在下载 ${progress}`:message||(pending?'下次启动时安装':'由你决定何时下载与安装')}</small></div>{action}<button disabled={busy} aria-label="稍后更新" onClick={()=>setDismissed(true)}><X size={14}/></button></aside>{modal}</>;
}
