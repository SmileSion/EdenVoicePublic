import type {CoreSnapshot,ChatMessage} from './types';

const KEY='eden.experience.v1';
type Room={draft?:string;read?:string;anchor?:string;muted?:boolean;updated:number};
type State={rooms:Record<string,Room>;dnd:boolean;sounds:boolean};
let cached:State|undefined;
let cachedRaw:string|null|undefined;
export const roomKey=(snapshot:Pick<CoreSnapshot,'serverAddress'|'announcementScope'|'clientId'>,channel:string)=>JSON.stringify([snapshot.serverAddress,snapshot.announcementScope||'',snapshot.clientId,channel]);
function read():State {
  try {
    const raw=localStorage.getItem(KEY);
    if(cached&&raw===cachedRaw)return cached;
    const parsed=JSON.parse(raw||'{}');
    const rooms:Record<string,Room>={};
    for(const [key,value] of Object.entries(parsed.rooms||{}).slice(-200)){
      const r=value as Room;
      if(r&&typeof r.updated==='number')rooms[key]={updated:r.updated,draft:typeof r.draft==='string'?r.draft.slice(0,2048):'',read:typeof r.read==='string'&&/^\d{1,20}$/.test(r.read)?r.read:undefined,anchor:typeof r.anchor==='string'?r.anchor:undefined,muted:r.muted===true};
    }
    cachedRaw=raw;cached={rooms,dnd:parsed.dnd===true,sounds:parsed.sounds!==false};return cached;
  }catch{return cached||{rooms:{},dnd:false,sounds:true};}
}
export function preferences(){const {dnd,sounds}=read();return {dnd,sounds};}
export function roomPreferences(key:string){return read().rooms[key];}
function save(state:State){
  state.rooms=Object.fromEntries(Object.entries(state.rooms).sort((a,b)=>b[1].updated-a[1].updated).slice(0,100));
  try{const raw=JSON.stringify(state);localStorage.setItem(KEY,raw);cachedRaw=raw;cached=state;}catch{cached=state;/* Keep an in-memory fallback if storage is full. */}
  window.dispatchEvent(new Event('eden-experience-changed'));
}
export function updateRoom(key:string,patch:Partial<Room>){const state=read();if(Object.entries(patch).every(([name,value])=>state.rooms[key]?.[name as keyof Room]===value))return;state.rooms[key]={...state.rooms[key],...patch,updated:Date.now()};save(state);}
export function updatePreferences(patch:Partial<Pick<State,'dnd'|'sounds'>>){save({...read(),...patch});}
export function newerMessage(id:string,read?:string){try{return /^\d{1,20}$/.test(id)&&BigInt(id)>BigInt(read||'0');}catch{return false;}}
export function compareMessages(a:ChatMessage,b:ChatMessage){
  if(/^\d{1,20}$/.test(a.id)&&/^\d{1,20}$/.test(b.id))return BigInt(a.id)<BigInt(b.id)?-1:a.id===b.id?0:1;
  return a.sentAtUnixMs-b.sentAtUnixMs;
}

export function diagnosticAdvice(snapshot:CoreSnapshot):Array<{title:string;detail:string;action:'audio'|'network'|'diagnostics'}>{
  const {audio,diagnostics:d}=snapshot;const tips:Array<{title:string;detail:string;action:'audio'|'network'|'diagnostics'}>=[];
  if(audio.state.startsWith('error')||audio.deviceError)tips.push({title:'音频设备暂不可用',detail:'检查系统麦克风权限、设备连接，再重试语音或选择可用设备。',action:'audio'});
  if(audio.dspError)tips.push({title:'语音处理异常',detail:'当前降噪或语音处理未正常工作。可在音频设置中切换日常降噪后重新试听。',action:'audio'});
  if([audio.inputSampleRate,audio.outputSampleRate].some(rate=>rate!==null&&rate>0&&rate<32000))tips.push({title:'设备采样率较低',detail:'蓝牙耳机可能进入通话模式。可尝试独立麦克风配合耳机输出，或改用有线设备。',action:'audio'});
  if(snapshot.connection==='connected'&&(d.timing?.workerP95Ms||0)>20)tips.push({title:'音频处理耗时偏高',detail:'处理耗时已超过 20 ms 音频帧时长，建议尝试日常降噪，并减少后台高负载任务。',action:'audio'});
  if(snapshot.connection==='connected'&&d.rttMs!==null&&(d.effectiveAudioLossPercent>3||d.jitterMs>30||d.rttMs>200))tips.push({title:'网络可能影响通话',detail:'检查 Wi-Fi 信号、下载任务和 VPN。先查看网络详情，对比故障发生时的丢包和延迟。',action:'network'});
  if(!tips.length)tips.push({title:'暂未发现明显异常',detail:'这些建议来自当前可用指标；如果仍有断音，可标记问题时间并导出诊断报告。',action:'diagnostics'});
  return tips;
}
