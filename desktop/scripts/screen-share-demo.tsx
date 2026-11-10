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
 const [admin,setAdmin]=useState(false);
 const [theme,setTheme]=useState<'dark'|'light'>('dark'),[room,setRoom]=useState('7');
 const channels=[{...mockSnapshot.channels[0],id:'7',name:'朋友的房间'},{...mockSnapshot.channels[0],id:'8',name:'深夜放映室'}];
 const users=['你','Lumi','小鹿','阿泽'].map((nickname,i)=>({...mockSnapshot.users[0],sessionId:String(100+i),clientId:'demo-'+i,nickname,avatar:'custom:demo-'+i,channelId:room||'7',speaking:i===2}));
 const snapshot={...mockSnapshot,theme,users,channels,connection:'connected' as const,selfSessionId:'100',serverAddress:'screen-demo',messages:[],watchSupported:false,screenShareSupported:true,capabilities:admin?['watch.manage']:[],roomScenes:{}};
 return <><header className="demo-controls"><strong>屏幕共享 · 交互预览</strong><button onClick={()=>w.mockController.scenario('idle')}>初始状态</button><button onClick={()=>w.mockController.scenario('invite')}>朋友发起共享</button><button onClick={()=>w.mockController.scenario('paused')}>朋友暂停中</button><button onClick={()=>w.mockController.friendPause()}>切换朋友暂停 / 恢复</button><button onClick={()=>w.mockController.scenario('ended')}>对方结束共享</button><button onClick={()=>w.mockController.scenario('error')}>连接失败</button><button onClick={()=>setAdmin(v=>!v)}>{admin?'管理员视角':'普通成员视角'}</button><button onClick={()=>setTheme(theme==='dark'?'light':'dark')}>切换明暗主题</button><small>本地模拟 · 不连接服务器</small></header><div className={'demo-client app-shell theme-'+theme}><div className="demo-title"><strong>EdenVoice</strong><span>本地交互预览</span></div><aside className="demo-sidebar"><small>当前服务器</small><h3>朋友的小天地</h3><button className={!room?'selected':''} onClick={()=>setRoom('')}>相聚之间</button><p>语音频道</p>{channels.map(c=><button key={c.id} onDoubleClick={()=>setRoom(c.id)}>◉ {c.name}<small>{users.filter(u=>u.channelId===c.id).length||''}</small></button>)}<footer>你 · 在线</footer></aside><div className="demo-main"><Chat snapshot={snapshot} channel={room?channels.find(c=>c.id===room):undefined} membersVisible={false} onToggleMembers={()=>{}} onMore={()=>{}}/></div><div className="demo-dock">● 语音房间示意 <small>朋友的房间</small><span style={{float:"right"}}>麦克风 · 耳机 · 输入 / 输出设备</span></div></div></>;
}
createRoot(document.getElementById('root')!).render(<Demo/>);
