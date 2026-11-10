import React, {useState} from 'react';
import {createRoot} from 'react-dom/client';
import {ArchiveJournal} from '../src/ArchiveTimeline';
import {ArchiveRecap} from '../src/ArchiveRecap';
import {Avatar} from '../src/Avatar';
import type {ArchiveEvent,RoomArchive} from '../src/types';
const names=['你','Lumi','小鹿','阿泽','Moon','薄荷','小林','River'];
const colors=['#9bb9ae','#baaad2','#a9bdd8','#d3b5a3','#a5bfc5','#b5c4a4','#cbb0bc','#aeb3d8'];
const images=names.map((_,i)=>'data:image/svg+xml,'+encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64"><rect width="64" height="64" rx="20" fill="${colors[i]}"/><circle cx="32" cy="27" r="12" fill="#f5e3c8"/><path d="M7 64Q8 40 32 40Q56 40 57 64" fill="${['#526d69','#735d8b','#536786','#957564'][i%4]}"/><path d="M19 25Q15 7 33 10Q50 9 45 28L39 20L21 24" fill="#4b4554"/></svg>`));
(window as any).__TAURI_INTERNALS__={};
(window as any).rpc=async(name:string,args:any)=>name==='get_avatar_data'?images[Number(String(args.reference).replace('custom:demo-',''))]||images[0]:name==='export_archive_card'?(()=>{const a=document.createElement('a');a.href='data:image/png;base64,'+args.image;a.download='Eden-Archive.png';a.click();return true;})():null;
const profiles=Object.fromEntries(names.map((name,i)=>['p'+i,{clientId:'p'+i,displayName:name,avatar:'custom:demo-'+i}]));
function game(id:string,title:string,day:number,hour:number,minutes:number,count:number):ArchiveEvent {
 const start=new Date(2026,8,day,hour,12),end=new Date(+start+minutes*60000);
 return {id:`${id}-${day}`,eventType:'shared_game',category:'game',occurredAt:start.toISOString(),triggeredBy:[],participants:names.slice(0,count).map((_,i)=>'p'+i),importance:55,rarity:'common',titleKey:'',descriptionKey:'',game:{game_id:id,title,started_at:start.toISOString(),ended_at:end.toISOString(),duration_seconds:minutes*60}};
}
const archive:RoomArchive={stats:{roomId:'42',roomName:'朋友的房间',createdAt:'2026-08-01',activeVoiceSeconds:197280,sessionCount:28,longestSessionSeconds:12600,maxConcurrentMembers:8,activeDays:19,historicalMembers:8},ageDays:45,enabled:true,hasMore:false,participantProfiles:profiles,events:[game('valorant','无畏契约',14,20,126,5),game('terraria','泰拉瑞亚',13,21,84,3),{id:'memory',eventType:'room_age_30d',category:'time',occurredAt:new Date(2026,8,13,19,0).toISOString(),triggeredBy:[],participants:['p0','p1','p2'],importance:60,rarity:'rare',titleKey:'',descriptionKey:''},game('cs2','Counter-Strike 2',12,20,97,8),game('valorant','无畏契约',11,20,63,4)]};
function copy(event:ArchiveEvent){return event.game?[`一起玩了 ${event.game.title}`,`${event.participants.length} 位朋友，在朋友的房间一起度过了 ${Math.floor(event.game.duration_seconds/60)} 分钟。`]:['在这里，相聚了一个月','从第一句「听得见吗」，到不必约定的每个晚上。'];}
function Demo(){const [theme,setTheme]=useState('dark'),[recap,setRecap]=useState<string|null>(null),[open,setOpen]=useState(true),[many,setMany]=useState(false);
 const data=many?{...archive,events:archive.events.map(e=>({...e,participants:Array.from({length:24},(_,i)=>'p'+i)}))}:archive;
 return <><div className="archive-demo-controls"><strong>Eden Archive · 共同游戏</strong><span>交互预览 / 模拟记录</span><button onClick={()=>setTheme(theme==='dark'?'light':'dark')}>{theme==='dark'?'切换白天':'切换夜晚'}</button><button onClick={()=>setMany(!many)}>{many?'普通人数':'体验多人记录'}</button><button onClick={()=>setOpen(true)}>打开档案</button></div><div className={`app-shell archive-demo-client theme-${theme}`}><header className="archive-demo-title"><b>EdenVoice</b><span>● 在线</span><span>—　□　×</span></header><aside className="archive-demo-sidebar"><small>当前服务器</small><h3>我们的空间</h3><p>语音频道</p><div>♧ 相聚之间</div><div className="chosen">◉ 朋友的房间 <small>5</small></div><div>◉ 深夜放映室</div><div>◉ 周末开黑</div><footer><Avatar avatar="custom:demo-0" name="你" size={30}/>你 <small>在线</small></footer></aside><main className="archive-demo-main"><h2>朋友的房间</h2><p>今晚也一起玩吧。</p><div className="archive-demo-window">相聚，不必匆忙</div></main><footer className="archive-demo-dock">● 语音已连接 <span>朋友的房间</span></footer>
 {open&&<div className="archive-backdrop"><ArchiveJournal archive={data} copy={copy} close={()=>setOpen(false)} onOverview={()=>setRecap('')} onRecap={e=>setRecap(e.id)}/></div>}
 {recap!==null&&<ArchiveRecap archive={data} initialEventId={recap} copy={copy} close={()=>setRecap(null)}/>}
 </div></>;
}
createRoot(document.getElementById('root')!).render(<Demo/>);
