import {useEffect,useRef,useState} from 'react';
import {Megaphone,X} from 'lucide-react';
import type {CoreSnapshot} from './types';
import {acknowledge,announcementKey,readRevision,validAnnouncement,type Announcement} from './announcementState';
export function AnnouncementNotice({snapshot}:{snapshot:CoreSnapshot}){
 if(snapshot.connection!=='connected'||!snapshot.announcementScope||!snapshot.clientId)return null;
 const scope=announcementKey(snapshot.announcementScope,snapshot.clientId);
 return <Notice key={scope} scope={scope} server={snapshot.serverName} version={snapshot.announcementRevision} incoming={snapshot.announcement}/>;
}
function readLocalRevision(scope:string){try{return readRevision(localStorage,scope);}catch{return 0n;}}
function Notice({scope,server,incoming,version}:{scope:string;server:string;version:string|undefined;incoming:Announcement|null|undefined}){
 const dialog=useRef<HTMLDialogElement>(null);
 const newest=useRef(0n);
 const [read,setRead]=useState(()=>readLocalRevision(scope));
 const [current,setCurrent]=useState<Announcement|null>(null);
 const [failure,setFailure]=useState(false);
 useEffect(()=>{
  if(!version||!/^[1-9]\d{0,19}$/.test(version))return;
  const stateRevision=BigInt(version);
  if(stateRevision<newest.current)return;
  if(incoming&&(!validAnnouncement(incoming)||BigInt(incoming.revision)>stateRevision))return;
  newest.current=stateRevision;
  setCurrent(incoming&&BigInt(incoming.revision)>read?incoming:null);
 },[incoming,read,version]);
 useEffect(()=>{const el=dialog.current;if(!el)return;if(current&&!el.open)el.showModal();if(!current&&el.open)el.close();},[current]);
 useEffect(()=>{setFailure(false);},[current?.revision]);
 useEffect(()=>{const sync=(e:StorageEvent)=>{if(e.key===scope)setRead(previous=>{const stored=readLocalRevision(scope);return stored>previous?stored:previous;});};window.addEventListener('storage',sync);return()=>window.removeEventListener('storage',sync);},[scope]);
 const close=()=>{if(!current)return;try{acknowledge(localStorage,scope,current.revision);}catch{if(!failure){setFailure(true);return;}}setRead(BigInt(current.revision));setCurrent(null);setFailure(false);};
 return <dialog ref={dialog} className="announcement-dialog" aria-labelledby="announcement-title" onCancel={e=>{e.preventDefault();close();}}>
  {current&&<><header><span className="announcement-icon"><Megaphone size={21}/></span><div><small>服务器公告</small><span>{server||'当前服务器'}</span></div><button aria-label="已读并关闭公告" onClick={close}><X size={18}/></button></header>
  <section><h2 id="announcement-title">{current.title}</h2><time>{new Date(current.published_at).toLocaleString('zh-CN',{hour12:false})}</time><div className="announcement-body">{current.body}</div></section>
  <footer><small>{failure?'无法保存已读记录，再次点击可关闭；下次连接可能再次提示。':'仅展示最新公告 · 阅读后不再提醒'}</small><button autoFocus onClick={close}>{failure?'仍然关闭':'知道了'}</button></footer></>}
 </dialog>;
}
