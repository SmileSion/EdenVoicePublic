import {useEffect,useState,useRef} from 'react';
import {X,LockKeyhole} from 'lucide-react';
import {dispatch} from './bridge';
import type {CoreSnapshot} from './types';
export function PasswordRoom({snapshot}:{snapshot:CoreSnapshot}){
 const submittedRevision=useRef(0);
 const [room,setRoom]=useState(''),[password,setPassword]=useState(''),[busy,setBusy]=useState(false),[error,setError]=useState('');
 useEffect(()=>{const open=(e:Event)=>{setRoom((e as CustomEvent<string>).detail);setPassword('');setBusy(false);setError('');};window.addEventListener('eden-password-room',open);return()=>window.removeEventListener('eden-password-room',open);},[]);
 useEffect(()=>{if(!room)return;if(snapshot.users.find(u=>u.sessionId===snapshot.selfSessionId)?.channelId===room){setRoom('');setPassword('');}else if(busy&&snapshot.revision>submittedRevision.current&&snapshot.error){setError(snapshot.error);setBusy(false);}},[snapshot,room,busy]);
 useEffect(()=>{if(!busy)return;const id=setTimeout(()=>{setBusy(false);setError('尚未进入房间，请检查连接后重试。');},8000);return()=>clearTimeout(id);},[busy]);
 if(!room)return null;
 return <div className="modal-backdrop compact-backdrop"><form className="channel-editor" aria-label="输入房间密码" onSubmit={async e=>{e.preventDefault();submittedRevision.current=snapshot.revision;setBusy(true);setError('');try{await dispatch({type:'join_protected_channel',channelId:room,password});}catch{setError('无法发送加入请求');setBusy(false);}}}><header><h2><LockKeyhole size={16}/> {snapshot.channels.find(c=>c.id===room)?.name||'加密房间'}</h2><button type="button" aria-label="取消输入密码" onClick={()=>{setRoom('');setPassword('');}}><X size={16}/></button></header><div className="channel-editor-fields"><label>房间密码<input autoFocus type="password" autoComplete="off" value={password} maxLength={72} onChange={e=>setPassword(e.target.value)}/></label>{error&&<p role="alert">{error}</p>}</div><footer><button type="button" onClick={()=>{setRoom('');setPassword('');}}>取消</button><button className="primary" disabled={busy||!password}>{busy?'正在加入…':'加入房间'}</button></footer></form></div>;
}
