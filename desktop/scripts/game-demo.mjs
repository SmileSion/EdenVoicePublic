// Real Chat component + isolated Go admin-demo. No production connections.
import {build} from 'esbuild';
import {readFile} from 'node:fs/promises';
import {createServer,request} from 'node:http';
import {fileURLToPath} from 'node:url';
const root=fileURLToPath(new URL('..',import.meta.url));
const bundle=await build({stdin:{resolveDir:root,loader:'tsx',contents:`
import React from 'react';import {createRoot} from 'react-dom/client';
import {Chat} from './src/App';import {mockSnapshot} from './src/mock';
const root=createRoot(document.getElementById('room'));
window.__TAURI_INTERNALS__={};window.subscribers=[];
let data,watch={managing:false,canControl:true,canClose:true,canTransferHost:false,role:'host',canReopen:false,active:false,loading:false,ready:false,roomOpen:false,hostId:'',selectedFile:'',status:'',channelId:'2',mpvPath:'demo',error:'',position:0,paused:true,roomPaused:true,duration:120,members:[]};
const notify=()=>window.subscribers.forEach(f=>f({payload:{...watch}}));
window.rpc=async(name,args)=>{
 if(name==='get_watch_snapshot')return {...watch};
 if(name==='get_avatar_data'){const r=await fetch('/api/avatar?reference='+encodeURIComponent(args.reference||args.avatar));if(r.ok)return (await r.json()).data;return '';}
 const c=args?.command;if(c?.type==='watch_room'){
  if(c.action==='open'){watch={...watch,roomOpen:true,active:true,hostId:String(data.state.users[0].session_id),members:data.state.users.map(u=>({sessionId:String(u.session_id),ready:false}))};notify();}
  if(c.action==='close'){watch={...watch,roomOpen:false,active:false,members:[]};notify();}
 }return null;
};
const safe=s=>String(s??'').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
async function refresh(){const r=await fetch('/demo/state');data=await r.json();window.demoState=data;
 const users=data.state.users.filter(u=>u.channel_id===2).map(u=>({...mockSnapshot.users[0],sessionId:String(u.session_id),clientId:u.client_id,nickname:u.nickname,avatar:u.avatar,channelId:'2',speaking:false,gameActivity:u.game_activity?{...u.game_activity,updatedAt:Date.now(),expiresAt:Date.now()+60000}:null}));
 root.render(<div className='app-shell theme-dark' style={{display:'block',height:'100%',width:'100%',minWidth:0}}><Chat snapshot={{...mockSnapshot,theme:'dark',serverName:'朋友的小天地',serverAddress:'isolated-demo',selfSessionId:users[0]?.sessionId,users,messages:[],watchSupported:true,roomScenes:data.state.room_scenes}} channel={{id:'2',name:'朋友的房间'}} membersVisible={false} onToggleMembers={()=>{}} onMore={()=>{}}/></div>);
 document.getElementById('version').textContent='目录 v'+data.catalog.version;
 document.getElementById('clock').textContent=new Date(data.now).toLocaleTimeString('zh-CN',{hour12:false});
 const focused=document.activeElement?.closest('#players');if(!focused)document.getElementById('players').innerHTML=users.map(u=>'<label><span>'+safe(u.nickname)+'</span><select data-member="'+u.sessionId+'"><option value="">没有在玩游戏</option>'+data.catalog.games.filter(g=>g.enabled).map(g=>'<option value="'+safe(g.id)+'" '+(data.state.users.find(p=>String(p.session_id)===u.sessionId).game_activity?.gameId===g.id?'selected':'')+'>'+safe(g.title)+'</option>').join('')+'</select></label>').join('');
}
document.getElementById('players').onchange=async e=>{await fetch('/demo/change',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({id:e.target.dataset.member,game:e.target.value})});e.target.blur();await refresh();};
document.querySelectorAll('[data-advance]').forEach(b=>b.onclick=async()=>{await fetch('/demo/change',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({advance:Number(b.dataset.advance)})});await refresh();});
await fetch('/api/login',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({username:'admin',password:'Eden-demo-games-2026'})});await refresh();setInterval(refresh,1500);
`},bundle:true,write:false,format:'esm',jsx:'automatic',loader:{'.svg':'text'},plugins:[{name:'demo-native',setup(b){b.onResolve({filter:/^@tauri-apps\/api\/(core|event|window)$/},a=>({path:a.path,namespace:'mock'}));b.onLoad({filter:/.*/,namespace:'mock'},()=>({contents:`export const invoke=(...a)=>window.rpc(...a);export const listen=async(n,f)=>{if(n==='watch-state')window.subscribers.push(f);return()=>window.subscribers=window.subscribers.filter(x=>x!==f)};export const getCurrentWindow=()=>({});`,loader:'js'}));}}]});
const css=(await Promise.all(['styles.css','watch-party.css','room-experience.css'].map(f=>readFile(root+'/src/'+f,'utf8')))).join('\n');
const html=`<!doctype html><html lang="zh-CN"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>EdenVoice · 自动游戏房间 Demo</title><link rel="stylesheet" href="/game-demo.css"><style>body{margin:0;background:#11131b;color:#eee;font:14px system-ui}*{box-sizing:border-box}.demo-top{padding:18px 24px;display:flex;align-items:center;gap:20px;border-bottom:1px solid #30333e}.demo-top a{margin-left:auto;color:#b5caff}#demo-shell{display:grid;grid-template-columns:215px minmax(0,1fr);max-width:1060px;margin:24px auto;background:#1a1c25;min-height:620px;border:1px solid #363844;border-radius:16px;overflow:hidden}.demo-aside{padding:24px 18px;border-right:1px solid #353743}.demo-aside h2{font-size:17px}.demo-aside p{color:#abaeba;line-height:1.8}.demo-room-link{background:#383345;color:#e5d6fa;padding:12px;border-radius:8px;margin:28px 0}.demo-aside label{display:block;margin:14px 0}.demo-aside label span{display:block;margin-bottom:5px}.demo-aside select{width:100%;background:#262935;color:#eee;border:1px solid #555869;border-radius:6px;padding:7px;font-size:12px}#room{height:650px;min-width:0}#room .chat-panel{height:100%}.demo-clock{display:flex;gap:12px;align-items:center;justify-content:center;padding-bottom:24px;flex-wrap:wrap}.demo-clock button{padding:9px 14px;border-radius:8px;background:#302d40;border:1px solid #68607d;color:#eee;cursor:pointer}small{color:#b3b4be}@media(max-width:740px){#demo-shell{margin:8px;grid-template-columns:160px minmax(0,1fr)}.demo-aside{padding:12px 8px}#room{height:680px}.demo-top{padding:12px;font-size:12px}}</style><header class="demo-top"><strong>EdenVoice</strong><small>本机演示 · 模拟成员 / 观影，真实房间判定与目录 API</small><a href="/console" target="_blank">打开游戏管理端 ↗</a></header><div id="demo-shell"><aside class="demo-aside"><small>朋友的小天地</small><h2>今晚也在这里</h2><div class="demo-room-link">◉ 朋友的房间</div><small id="version"></small><div id="players"></div><p>更改成员游戏状态<br>3 / 5 人同款即可触发</p></aside><main id="room"></main></div><div class="demo-clock"><small>演示时钟 <span id="clock"></span></small><button data-advance="15">快进 15 秒</button><button data-advance="30">快进 30 秒</button><small>生产环境使用真实时间</small></div><script type="module" src="/game-demo.js"></script></html>`;
createServer((req,res)=>{
 if(req.url==='/'){res.setHeader('Content-Type','text/html; charset=utf-8');res.end(html);return;}
 if(req.url==='/game-demo.js'){res.setHeader('Content-Type','text/javascript');res.end(bundle.outputFiles[0].text);return;}
 if(req.url==='/game-demo.css'){res.setHeader('Content-Type','text/css');res.end(css);return;}
 const upstream=request({hostname:'127.0.0.1',port:9181,path:req.url==='/console'?'/':req.url,method:req.method,headers:{...req.headers,host:'127.0.0.1:9181',origin:'http://127.0.0.1:9181'}},r=>{res.writeHead(r.statusCode,r.headers);r.pipe(res)});upstream.on('error',()=>{res.statusCode=502;res.end('Start the isolated admin-demo first.');});req.pipe(upstream);
}).listen(9182,'127.0.0.1',()=>console.log('Game demo: http://127.0.0.1:9182 (isolated local data)'));
