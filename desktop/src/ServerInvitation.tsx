import {useEffect,useRef,useState} from 'react';
import {invoke} from '@tauri-apps/api/core';
import {dispatch,getSnapshot} from './bridge';
import type {CoreSnapshot} from './types';

// Opening a URI only prepares this confirmation. It never joins or trusts a server.
export function ServerInvitation({snapshot}:{snapshot:CoreSnapshot}) {
 const [address,setAddress]=useState<string|null>(null),[nickname,setNickname]=useState(''),[password,setPassword]=useState(''),[error,setError]=useState(''),[submitting,setSubmitting]=useState(false);
 const dialog=useRef<HTMLDialogElement>(null),busy=useRef(false),pending=useRef(false);pending.current=!!address;
 useEffect(()=>{const open=(event:Event)=>{const profile=(event as CustomEvent<{address:string;nickname?:string}>).detail;if(busy.current||pending.current||!profile?.address)return;pending.current=true;setAddress(profile.address);setNickname(profile.nickname||snapshot.nickname);setPassword('');setError('');};window.addEventListener('eden-open-saved-server',open);return()=>window.removeEventListener('eden-open-saved-server',open);},[snapshot.nickname]);
 useEffect(()=>{if(!('__TAURI_INTERNALS__' in window))return;let gone=false;const check=async()=>{if(busy.current||pending.current)return;busy.current=true;try{const next=await invoke<string|null>('take_server_invitation');if(next&&!gone){pending.current=true;setAddress(next);setNickname(snapshot.nickname);setPassword('');setError('');}}finally{busy.current=false;}};void check().catch(()=>{});const timer=setInterval(()=>void check().catch(()=>{}),1500);return()=>{gone=true;clearInterval(timer);};},[snapshot.nickname]);
 useEffect(()=>{if(address&&!dialog.current?.open)dialog.current?.showModal();},[address]);
 const close=()=>{dialog.current?.close();setAddress(null);setPassword('');};
 if(!address)return null;
 const connected=['connected','reconnecting','connecting'].includes(snapshot.connection);
 return <dialog className="server-invitation" ref={dialog} onCancel={e=>{if(submitting)e.preventDefault();else close();}}><form onSubmit={async e=>{e.preventDefault();if(!nickname.trim()||busy.current)return;busy.current=true;setSubmitting(true);setError('');try{if(connected){await dispatch({type:'disconnect'});let stopped=false;for(let i=0;i<100;i++){const state=await getSnapshot();if(state.connection==='disconnected'){stopped=true;break;}await new Promise(r=>setTimeout(r,100));}if(!stopped)throw new Error('disconnect timeout');}await dispatch({type:'connect',address,nickname:nickname.trim(),serverPassword:password,remember:false});close();}catch{setError('未能发起连接，请稍后重试。');}finally{busy.current=false;setSubmitting(false);}}}><h2>朋友邀请你加入服务器</h2><p>请确认这是你想连接的地址。服务器身份仍需单独验证。</p><code>{address}</code><label>昵称<input autoFocus value={nickname} maxLength={64} onChange={e=>setNickname(e.target.value)}/></label><label>服务器密码<input type="password" autoComplete="off" maxLength={256} value={password} onChange={e=>setPassword(e.target.value)} placeholder="无需密码则留空"/></label>{connected&&<p className="invite-warning">确认后会结束当前服务器的连接与通话，再连接邀请的服务器。</p>}{error&&<p role="alert">{error}</p>}<footer><button type="button" disabled={submitting} onClick={close}>取消</button><button className="primary" disabled={!nickname.trim()||submitting}>{submitting?"正在切换…":"确认并连接"}</button></footer></form></dialog>;
}
