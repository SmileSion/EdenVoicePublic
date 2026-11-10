import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from '../src/App';
import { mockSnapshot } from '../src/mock';
import { emptyWatch } from '../src/useWatchRoom';
import type { CoreSnapshot, CoreCommand, RoomSocialContext, RoomSocialEvent, RoomSocialRequest, RoomImageMetadata } from '../src/types';
import '../src/styles.css';
import '../src/watch-party.css';
import '../src/room-experience.css';
import '../src/close-behavior.css';

const w = window as any;
w.__TAURI_INTERNALS__ = {};
const listeners = new Map<string, Set<(event: {payload: any}) => void>>();
w.nativeListen = async (name: string, callback: (event: {payload: any}) => void) => {
  const entries = listeners.get(name) ?? new Set(); entries.add(callback); listeners.set(name, entries);
  return () => entries.delete(callback);
};
const emit = (name: string, payload: unknown) => listeners.get(name)?.forEach(callback => callback({payload: structuredClone(payload)}));
const avatars = ['🐱', '🦊', '🐼', '🐻', '🐰', '🐸', '🐵', '🦁'];
const names = ['你', 'Lumi', '小鹿', '阿泽', '薄荷', 'Moon', '小林', '栗子'];
const makeUsers = (count: number, room = '2') => Array.from({length: count}, (_, index) => ({...mockSnapshot.users[0], sessionId: String(9001 + index), clientId: `local-friend-${index}`, nickname: `${names[index % names.length]}${index >= names.length ? ` ${Math.floor(index / names.length) + 1}` : ''}`, avatar: avatars[index % avatars.length], channelId: index ? '2' : room, speaking: false, muted: false, adminMuted: false, serverRole: index ? 'USER' : 'MASTER', channelRole: ''}));
let state: CoreSnapshot = {...structuredClone(mockSnapshot), serverName: '朋友的小天地 · 本地模拟', serverAddress: 'local-demo.invalid:7000', announcementScope: 'local-demo-fingerprint', announcement: null, announcementRevision: '0', selfSessionId: '9001', clientId: 'local-friend-0', nickname: '你', avatar: '🐱', roomSocialSupported: true, roomImagesSupported: true, roomSocialEpoch: 'local-demo-1', screenShareSupported: false, watchSupported: true, archiveSupported: false, inputLevel: 0, outputLevel: 0, muted: false, deafened: false, selfAway: false, mutedSpeechNotice: null, users: makeUsers(4), channels: [{...mockSnapshot.channels[0], id:'2', name:'夜航小队', parentId:'0'}, {...mockSnapshot.channels[0], id:'3', name:'咖啡角', parentId:'0', sortOrder:10}], messages: [
  {id:'welcome-1', channelId:'2', senderSessionId:'9002', senderClientId:'local-friend-1', senderName:'Lumi', senderAvatar:'🦊', text:'今晚不赶时间，坐下聊会儿。', sentAtUnixMs:Date.now()-180000},
  {id:'welcome-2', channelId:'2', senderSessionId:'9003', senderClientId:'local-friend-2', senderName:'小鹿', senderAvatar:'🐼', text:'等下把刚才那局的截图发来看看 ☕', sentAtUnixMs:Date.now()-120000},
]};
const currentRoom = () => state.users.find(user => user.sessionId === state.selfSessionId)?.channelId || '0';
const publish = () => { state = {...state, revision: state.revision + 1}; emit('core-state', state); emit('demo-state', {theme: state.theme, count: state.users.length, room: currentRoom()}); };
const away = new Map<string, Set<string>>();
type Upload = {room: string; size: number; width: number; height: number; chunks: Uint8Array[]; offset: number};
const uploads = new Map<string, Upload>();
const images = new Map<string, {room: string; bytes: Uint8Array; meta: RoomImageMetadata}>();
let sequence = 0, slowUploads = false;
const calls: {name: string; action?: string; channelId?: string; offset?: number}[] = [];
const toBase64 = (bytes: Uint8Array) => { let value = ''; for (const byte of bytes) value += String.fromCharCode(byte); return btoa(value); };
const fromBase64 = (value: string) => Uint8Array.from(atob(value), character => character.charCodeAt(0));
const context = (): RoomSocialContext => ({serverAddress: state.serverAddress, serverFingerprint: state.announcementScope, selfSessionId: state.selfSessionId, channelId: currentRoom()});
const envelope = (ctx: RoomSocialContext, request: RoomSocialRequest, change: Partial<RoomSocialEvent>): RoomSocialEvent => ({request_id:request.request_id, channel_id:ctx.channelId, server_address:ctx.serverAddress, server_fingerprint:ctx.serverFingerprint, self_session_id:ctx.selfSessionId, kind:'status', ...change});
async function socialRpc(ctx: RoomSocialContext, request: RoomSocialRequest): Promise<RoomSocialEvent> {
  const response = (change: Partial<RoomSocialEvent>) => envelope(ctx, request, change);
  if (request.action === 'image_cancel') { uploads.delete(request.upload_id || ''); return response({kind:'image_cancelled',upload_id:request.upload_id}); }
  if (ctx.serverAddress !== state.serverAddress || ctx.selfSessionId !== state.selfSessionId || ctx.serverFingerprint !== state.announcementScope || ctx.channelId !== currentRoom() || request.channel_id !== ctx.channelId) return response({kind:'error',code:'context_changed',message:'本地模拟：房间已切换'});
  if (request.action === 'status') {
    const event = response({kind:'status',away:[...(away.get(ctx.channelId) ?? [])].map(session_id=>({session_id}))}); emit('room-social',event); return event;
  }
  if (request.action === 'away') {
    const roomAway = away.get(ctx.channelId) ?? new Set<string>();
    if (request.active) roomAway.add(ctx.selfSessionId); else roomAway.delete(ctx.selfSessionId);
    away.set(ctx.channelId,roomAway);
    state={...state,selfAway:!!request.active,muted:request.active ? true : state.muted,users:state.users.map(user=>user.sessionId===state.selfSessionId?{...user,muted:request.active ? true : user.muted}:user)};publish();
    const event = response({kind:'away',session_id:ctx.selfSessionId,active:request.active}); emit('room-social',event); return event;
  }
  if (request.action === 'reaction') { const event=response({kind:'reaction',session_id:ctx.selfSessionId,emoji:request.emoji,expires_at_unix_ms:Date.now()+2500}); emit('room-social',event); return event; }
  if (request.action === 'image_begin') {
    const id = `local-upload-${++sequence}`;
    uploads.set(id,{room:ctx.channelId,size:request.bytes!,width:request.width!,height:request.height!,chunks:[],offset:0});
    return response({kind:'image_upload',upload_id:id,offset:0});
  }
  if (request.action === 'image_chunk') {
    if (slowUploads) await new Promise(resolve=>setTimeout(resolve,650));
    const upload=uploads.get(request.upload_id || '');
    if (!upload || upload.room!==ctx.channelId || upload.offset!==request.offset) return response({kind:'error',code:'upload_invalid',message:'本地模拟上传已取消'});
    const bytes=fromBase64(request.data || '');
    if (!bytes.length || bytes.length>8192 || upload.offset+bytes.length>upload.size) return response({kind:'error',code:'chunk_invalid',message:'本地模拟分块无效'});
    upload.chunks.push(bytes);upload.offset+=bytes.length;
    return response({kind:'image_upload',upload_id:request.upload_id,offset:upload.offset});
  }
  if (request.action === 'image_commit') {
    const upload=uploads.get(request.upload_id || '');
    if (!upload || upload.offset!==upload.size || upload.room!==ctx.channelId) return response({kind:'error',code:'upload_invalid',message:'图片未传完'});
    const bytes = new Uint8Array(upload.size);let offset=0;
    for (const chunk of upload.chunks) { bytes.set(chunk,offset);offset+=chunk.length; }
    const reference=`local-image-${++sequence}`;
    const meta:RoomImageMetadata={reference,mime:'image/jpeg',width:upload.width,height:upload.height,bytes:bytes.length};
    images.set(reference,{room:ctx.channelId,bytes,meta});uploads.delete(request.upload_id!);
    state={...state,messages:[...state.messages,{id:`local-message-${++sequence}`,channelId:ctx.channelId,senderSessionId:state.selfSessionId,senderClientId:state.clientId,senderName:state.nickname,senderAvatar:state.avatar,text:'[图片]',sentAtUnixMs:Date.now(),image:meta}]};publish();
    return response({kind:'image_committed',upload_id:request.upload_id,image:meta,reference});
  }
  if (request.action === 'image_get') {
    const image=images.get(request.reference || ''),offset=request.offset || 0;
    if (!image || image.room!==ctx.channelId || offset>=image.bytes.length) return response({kind:'error',code:'not_found',message:'本地模拟：图片不在当前房间'});
    const bytes=image.bytes.subarray(offset,offset+8192);
    return response({kind:'image_data',reference:request.reference,offset,data:toBase64(bytes),eof:offset+bytes.length===image.bytes.length});
  }
  return response({kind:'error',code:'unsupported',message:'本地模拟未启用此操作'});
}
function coreDispatch(command: CoreCommand) {
  if (command.type==='join_channel' || command.type==='leave_channel') {
    const oldRoom=currentRoom(),next=command.type==='join_channel'?command.channelId:'0';
    away.get(oldRoom)?.delete(state.selfSessionId);
    state={...state,selfAway:false,mutedSpeechNotice:null,roomSocialEpoch:String(++sequence),users:state.users.map(user=>user.sessionId===state.selfSessionId?{...user,channelId:next}:user)};publish();return;
  }
  if (command.type==='send_text_message') {
    state={...state,messages:[...state.messages,{id:`local-message-${++sequence}`,channelId:currentRoom(),senderSessionId:state.selfSessionId,senderClientId:state.clientId,senderName:state.nickname,senderAvatar:state.avatar,text:command.text,sentAtUnixMs:Date.now()}]};publish();return;
  }
  if (command.type==='set_muted') { state={...state,muted:command.muted,selfAway:command.muted?state.selfAway:false,users:state.users.map(user=>user.sessionId===state.selfSessionId?{...user,muted:command.muted}:user)};publish();if(!command.muted){away.get(currentRoom())?.delete(state.selfSessionId);emit('room-social',envelope(context(),{action:'away',channel_id:currentRoom()},{kind:'away',session_id:state.selfSessionId,active:false}));} }
  if (command.type==='set_deafened') { state={...state,deafened:command.deafened};publish(); }
  if (command.type==='set_theme') { state={...state,theme:command.theme};publish(); }
}
w.rpc = async (name: string, args: any = {}) => {
  calls.push({name,action:args.request?.action||args.command?.type,channelId:args.context?.channelId,offset:args.request?.offset});
  if (name==='get_snapshot') return structuredClone(state);
  if (name==='dispatch_core') return coreDispatch(args.command);
  if (name==='request_room_social') return socialRpc(args.context,args.request);
  if (name==='get_watch_snapshot') return {...emptyWatch,channelId:currentRoom()};
  if (name==='get_close_preferences') return {behavior:'tray',pending:false,trayAvailable:true};
  if (name==='resize_member_panel') return false;
  return null;
};
function friendReaction() {
  const members=state.users.filter(user=>user.channelId===currentRoom()&&user.sessionId!==state.selfSessionId);
  const chosen = members.length>8 ? [members[0],...members.slice(-3)] : members;
  chosen.forEach((user,index)=>emit('room-social',envelope(context(),{action:'reaction',channel_id:currentRoom()},{kind:'reaction',session_id:user.sessionId,emoji:['🎉','👍','❤️','👏'][index%4],expires_at_unix_ms:Date.now()+2500})));
}
function friendAway() {
  const user=state.users.find(user=>user.channelId===currentRoom()&&user.sessionId!==state.selfSessionId);if(!user)return;
  const roomAway=away.get(currentRoom())??new Set<string>(),active=!roomAway.has(user.sessionId);
  if(active)roomAway.add(user.sessionId);else roomAway.delete(user.sessionId);away.set(currentRoom(),roomAway);
  state={...state,users:state.users.map(member=>member.sessionId===user.sessionId?{...member,muted:active}:member)};publish();
  emit('room-social',envelope(context(),{action:'away',channel_id:currentRoom()},{kind:'away',session_id:user.sessionId,active}));
}
async function sampleFile() {
  const canvas=document.createElement('canvas');canvas.width=1440;canvas.height=900;const ctx=canvas.getContext('2d')!;
  const gradient=ctx.createLinearGradient(0,0,1440,900);gradient.addColorStop(0,'#172b40');gradient.addColorStop(1,'#14131f');ctx.fillStyle=gradient;ctx.fillRect(0,0,1440,900);
  ctx.strokeStyle='#314c60';ctx.lineWidth=1;for(let i=0;i<36;i++){ctx.beginPath();ctx.moveTo(i*75-550,0);ctx.lineTo(i*75,900);ctx.stroke();}
  ctx.fillStyle='#193b48';ctx.beginPath();ctx.moveTo(840,0);ctx.lineTo(1360,0);ctx.lineTo(950,900);ctx.lineTo(440,900);ctx.fill();
  ctx.fillStyle='#92e5db';ctx.font='600 23px system-ui';ctx.fillText('EDENVOICE  /  LOCAL DEMO',80,76);
  ctx.font='800 96px system-ui';ctx.fillStyle='#f2faf6';ctx.fillText('VICTORY',76,215);
  ctx.font='30px system-ui';ctx.fillStyle='#aec2cb';ctx.fillText('周末小队 · 虚拟战绩示意',80,273);
  ctx.font='700 112px system-ui';ctx.fillStyle='#92e5db';ctx.fillText('13',80,420);ctx.fillStyle='#76859a';ctx.fillText('—',235,414);ctx.fillStyle='#faf0d9';ctx.fillText('8',390,420);
  ctx.fillStyle='#102433';ctx.fillRect(76,490,1280,274);
  const labels=['你','Lumi','小鹿','阿泽'];labels.forEach((name,index)=>{const x=108+index*310;ctx.fillStyle=['#acc7ff','#cbb5eb','#a9d5c0','#edc79e'][index];ctx.beginPath();ctx.arc(x+38,551,24,0,Math.PI*2);ctx.fill();ctx.fillStyle='#ebf0f3';ctx.font='600 28px system-ui';ctx.fillText(name,x,620);ctx.font='18px system-ui';ctx.fillStyle='#9bafbd';ctx.fillText(['22 / 12 / 6','18 / 11 / 9','16 / 13 / 7','15 / 12 / 8'][index],x,663);ctx.font='15px system-ui';ctx.fillText('一起玩的好时光',x,710);});
  ctx.fillStyle='#8797a9';ctx.font='18px system-ui';ctx.fillText('本地生成的演示图片 · 非真实游戏截图或玩家战绩',80,840);
  const blob=await new Promise<Blob>(resolve=>canvas.toBlob(value=>resolve(value!),'image/png'));
  return new File([blob],'local-demo-match.png',{type:'image/png',lastModified:Date.now()});
}
async function pasteSample() {
  const input=document.querySelector<HTMLInputElement>('.composer>input:not([type=file])');if(!input||input.disabled)return;
  const transfer=new DataTransfer();transfer.items.add(await sampleFile());input.focus();input.dispatchEvent(new ClipboardEvent('paste',{bubbles:true,cancelable:true,clipboardData:transfer}));
}
function mutedSpeech(){if(currentRoom()==='0'||state.selfAway)return;coreDispatch({type:'set_muted',muted:true});state={...state,mutedSpeechNotice:{sequence:String(++sequence),atUnixMs:Date.now()}};publish();}
w.roomSocialDemo={calls,emit,friendReaction,friendAway,pasteSample,mutedSpeech,coreDispatch,snapshot:()=>structuredClone(state),setSlowUploads:(value:boolean)=>{slowUploads=value;},activeUploads:()=>uploads.size,images:()=>images.size,join:(channelId:string)=>coreDispatch({type:'join_channel',channelId})};
function Demo() {
  const [view,setView]=useState({theme:state.theme,count:state.users.length,room:currentRoom()});
  React.useEffect(()=>{let stop=()=>{};void w.nativeListen('demo-state',(event:any)=>setView(event.payload)).then((value:()=>void)=>{stop=value;});return()=>stop();},[]);
  return <div className="social-demo"><header className="social-demo-controls"><div className="social-demo-heading"><strong>朋友的房间 · 互动预览</strong><small>本地模拟 · 不连接服务器</small></div><label>主题<select aria-label="演示主题" value={view.theme} onChange={event=>{state={...state,theme:event.target.value as 'dark'|'light'};publish();}}><option value="dark">夜晚</option><option value="light">白天</option></select></label><label>人数<select aria-label="演示人数" value={view.count} onChange={event=>{state={...state,users:makeUsers(Number(event.target.value),currentRoom())};publish();}}>{[4,16,40].map(count=><option key={count} value={count}>{count} 人</option>)}</select></label><label>房间<select aria-label="演示房间" value={view.room} onChange={event=>coreDispatch({type:'join_channel',channelId:event.target.value})}><option value="2">夜航小队</option><option value="3">咖啡角</option><option value="0">大厅</option></select></label><button onClick={friendReaction}>模拟朋友回应</button><button onClick={friendAway}>模拟朋友暂离</button><button onClick={mutedSpeech} disabled={view.room==='0'}>模拟静音说话</button><button className="demo-paste" onClick={()=>void pasteSample()} disabled={view.room==='0'}>试贴一张截图</button></header><div className="social-demo-frame"><App/></div><footer className="social-demo-footer">真实客户端组件与本地事件模拟 · 截图须预览确认后发送 · 所有记录仅保留在当前浏览器页</footer></div>;
}
createRoot(document.getElementById('root')!).render(<Demo/>);
