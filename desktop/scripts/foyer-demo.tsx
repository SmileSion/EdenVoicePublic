import React,{useState} from 'react';
import {createRoot} from 'react-dom/client';
import {Chat} from '../src/App';
import {mockSnapshot} from '../src/mock';
const w=window as any;
w.__TAURI_INTERNALS__={};w.demoHour=18;
const RealDate=Date;
w.Date=class extends RealDate {getHours(){return w.demoHour;}};
w.rpc=async(name:string)=>name==='get_avatar_data'?'data:image/svg+xml,'+encodeURIComponent('<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64"><rect width="64" height="64" rx="20" fill="#91a7a3"/><circle cx="32" cy="26" r="12" fill="#ead7be"/><path d="M9 64V54Q12 38 32 39Q53 38 55 54V64" fill="#565b7a"/><path d="M20 24Q16 9 31 10Q49 8 45 26L39 19L21 23" fill="#494156"/></svg>'):null;
function Demo(){
 const [count,setCount]=useState(8),[active,setActive]=useState(1),[theme,setTheme]=useState('dark'),[hour,setHour]=useState(18),[room,setRoom]=useState('');
 const games=['三角洲行动','无畏契约','泰拉瑞亚','黎明杀机'];
 const channels=games.map((_,i)=>({...mockSnapshot.channels[0],id:String(i+2),name:['深夜开黑','周末小队','像素花园','月下同行'][i]}));
 const n=Math.max(count,active*3+1);
 const users=Array.from({length:n},(_,i)=>({...mockSnapshot.users[0],sessionId:String(100+i),clientId:'demo-'+i,nickname:i===0?'你':['Lumi','阿泽','小鹿','Moon','小林','薄荷'][((i-1)%6)]+(i>6?' '+i:''),avatar:'custom:demo-'+i,channelId:i>0&&i<=active*3?String(2+Math.floor((i-1)/3)):'0',speaking:false}));
 const snapshot={...mockSnapshot,theme,users,channels,connection:'connected' as const,selfSessionId:'100',serverAddress:'foyer-demo',messages:[],watchSupported:false,roomScenes:Object.fromEntries(channels.slice(0,active).map((c,i)=>[c.id,{game_id:'demo-game-'+i,title:games[i]}]))};
 return <><header className="demo-controls"><strong>相聚之间 · 验收预览</strong><label>在线人数<select aria-label="在线人数" value={count} onChange={e=>{setCount(+e.target.value);if(+e.target.value===1)setActive(0);}}>{[1,8,24,80].map(n=><option key={n}>{n}</option>)}</select></label><label>游戏房间<select aria-label="游戏房间" value={active} onChange={e=>{const next=+e.target.value;setActive(next);if(count<next*3+1)setCount(next>=3?24:8);}}>{[0,1,3,4].map(n=><option key={n}>{n}</option>)}</select></label><label>光线<select aria-label="光线" value={hour} onChange={e=>{w.demoHour=+e.target.value;setHour(+e.target.value);document.dispatchEvent(new Event('visibilitychange'));}}><option value={10}>白天</option><option value={18}>傍晚</option><option value={23}>夜晚</option></select></label><button onClick={()=>setTheme(theme==='dark'?'light':'dark')}>切换明暗主题</button><small>模拟数据 · 实际客户端组件</small></header><div className={'demo-client app-shell theme-'+theme}><div className="demo-title"><strong>EdenVoice</strong><span>● 在线 · 未加入语音</span></div><aside className="demo-sidebar"><small>当前服务器</small><h3>朋友的小天地</h3><button className={!room?'selected':''} onClick={()=>setRoom('')}>相聚之间</button><p>语音频道</p>{channels.map(c=><button key={c.id} onDoubleClick={()=>setRoom(c.id)}>◉ {c.name}<small>{users.filter(u=>u.channelId===c.id).length||''}</small></button>)}<footer>你 · 在线</footer></aside><div className="demo-main"><Chat snapshot={snapshot} channel={room?channels.find(c=>c.id===room):undefined} membersVisible={false} onToggleMembers={()=>{}} onMore={()=>{}}/></div><div className="demo-dock">尚未加入语音 <small>左侧双击房间进入布局预览 · 不建立真实通话</small></div></div></>;
}
createRoot(document.getElementById('root')!).render(<Demo/>);
