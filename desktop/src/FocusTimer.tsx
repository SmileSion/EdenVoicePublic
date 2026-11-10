import {useEffect,useRef,useState} from 'react';
import {Timer,Play,Pause,RotateCcw,X,Plus,Trash2,ChevronLeft,ChevronRight} from 'lucide-react';
import {elapsedAt,formatTimer,parseTimer,timerDefault,timerDone,timerKey,timerValue,type TimerState} from './focus-timer-state';
import {playUiSound,prepareUiAudio} from './uiSounds';

type Entry=TimerState&{id:string;title:string};
const listKey='eden.focus-timers.v2';
const newEntry=():Entry=>({...timerDefault,id:crypto.randomUUID(),title:''});
function loadEntries():Entry[]{
  try{
    const raw=localStorage.getItem(listKey);
    if(raw){const rows=JSON.parse(raw);if(Array.isArray(rows)){const ids=new Set<string>();let hasRunning=false;return rows.filter(r=>r&&typeof r.id==='string'&&!ids.has(r.id)&&!!ids.add(r.id)).slice(0,10).map(r=>{const t={...parseTimer(JSON.stringify(r)),id:r.id,title:typeof r.title==='string'?r.title.slice(0,40):''};if(t.startedAt!==null){if(hasRunning)return {...t,elapsed:elapsedAt(t),startedAt:null};hasRunning=true;}return t;});}}
    const legacy=localStorage.getItem(timerKey);return [legacy?{...parseTimer(legacy),id:crypto.randomUUID(),title:''}:newEntry()];
  }catch{return [newEntry()];}
}
export function FocusTimer(){
  const [entries,setEntries]=useState(loadEntries),[open,setOpen]=useState(false),[now,setNow]=useState(Date.now),[error,setError]=useState(''),[selected,setSelected]=useState(0);
  const root=useRef<HTMLDivElement>(null),played=useRef(new Set<string>());
  const running=entries.some(t=>t.startedAt!==null);
  useEffect(()=>{try{localStorage.setItem(listKey,JSON.stringify(entries));}catch{setError('无法保存计时，关闭应用后可能丢失。');}},[entries]);
  useEffect(()=>{if(!running)return;const tick=()=>setNow(Date.now());const id=window.setInterval(tick,1000);document.addEventListener('visibilitychange',tick);window.addEventListener('focus',tick);return()=>{clearInterval(id);document.removeEventListener('visibilitychange',tick);window.removeEventListener('focus',tick);};},[running]);
  useEffect(()=>{
    const completed=entries.filter(t=>t.startedAt!==null&&timerDone(t,now));if(!completed.length)return;
    let ding=false;for(const t of completed){const key=t.id+':'+t.startedAt;if(!played.current.has(key)){played.current.add(key);ding=true;}}
    if(ding)void playUiSound('timer-done');
    setEntries(rows=>rows.map(t=>completed.some(c=>c.id===t.id)?{...t,elapsed:t.duration,startedAt:null}:t));
  },[entries,now]);
  useEffect(()=>{if(!open)return;const outside=(e:PointerEvent)=>{if(!root.current?.contains(e.target as Node))setOpen(false);};const key=(e:KeyboardEvent)=>{if(e.key==='Escape'){setOpen(false);root.current?.querySelector<HTMLButtonElement>('.focus-timer-chip')?.focus();}};document.addEventListener('pointerdown',outside);document.addEventListener('keydown',key);return()=>{document.removeEventListener('pointerdown',outside);document.removeEventListener('keydown',key);};},[open]);
  const update=(id:string,change:Partial<Entry>)=>setEntries(rows=>rows.map(t=>t.id===id?{...t,...change}:t));
  const toggle=(t:Entry)=>{const time=Date.now();if(t.startedAt!==null&&!timerDone(t,time))update(t.id,{elapsed:elapsedAt(t,time),startedAt:null});else{if(t.mode==='down'&&t.duration<1000){setError('倒计时至少设置 1 秒。');return;}void prepareUiAudio();setEntries(rows=>rows.map(r=>r.id===t.id?{...r,elapsed:timerDone(r,time)?0:r.elapsed,startedAt:time}:r.startedAt!==null?{...r,elapsed:elapsedAt(r,time),startedAt:null}:r));}setError('');setNow(time);};
  const active=entries.filter(t=>t.startedAt!==null&&!timerDone(t,now)),finished=entries.filter(t=>timerDone(t,now)).length;
  const page=Math.min(selected,Math.max(0,entries.length-1));
  const turn=(delta:number)=>{setSelected((page+delta+entries.length)%entries.length);setError('');};
  const featured=active.find(t=>t.mode==='down')||active[0];
  return <div ref={root} className="focus-timer" onPointerDown={e=>e.stopPropagation()}>
    <button className={`focus-timer-chip ${finished?'done':''}`} aria-label="番茄钟" aria-expanded={open} title={`计时器 · ${active.length} 个运行中${finished?` · ${finished} 个已完成`:''}`} onClick={()=>{setNow(Date.now());setOpen(!open);}}><Timer size={15}/>{featured?<span>{formatTimer(timerValue(featured,now))}{active.length>1?` +${active.length-1}`:''}</span>:finished?<span>已完成 {finished}</span>:null}</button>
    {open&&<section className="focus-timer-popover" aria-label="番茄钟面板" onKeyDown={e=>{if((e.target as HTMLElement).closest('input,select')||entries.length<2)return;if(e.key==='ArrowLeft'||e.key==='ArrowRight'){e.preventDefault();e.stopPropagation();turn(e.key==='ArrowLeft'?-1:1);}}}><header><strong>计时器</strong><div><button aria-label="添加计时" title={entries.length>=10?'最多保存 10 个计时':'添加计时'} disabled={entries.length>=10} onClick={()=>{if(entries.length>=10)return;setEntries(rows=>rows.length<10?[...rows,newEntry()]:rows);setSelected(entries.length);setError('');}}><Plus size={14}/></button><button aria-label="收起番茄钟" onClick={()=>setOpen(false)}><X size={14}/></button></div></header><div className="focus-timer-carousel">
    {entries.slice(page,page+1).map((t)=>{const index=page;const done=timerDone(t,now),busy=t.startedAt!==null&&!done,locked=busy||t.elapsed>0;const seconds=Math.floor(t.duration/1000);return <article className="focus-timer-entry" key={t.id} aria-label={`计时 ${index+1}`}><div className="focus-timer-entry-head"><input className="focus-timer-title" aria-label={`计时 ${index+1} 标题`} placeholder={`计时 ${index+1}`} maxLength={40} value={t.title} onChange={e=>update(t.id,{title:e.target.value})}/><button aria-label={`删除计时 ${index+1}`} onClick={()=>{setEntries(rows=>rows.filter(r=>r.id!==t.id));setSelected(Math.max(0,page-1));setError('');}}><Trash2 size={12}/></button></div><div className="focus-timer-entry-main"><output className="focus-timer-display">{formatTimer(timerValue(t,now))}</output><button aria-label={busy?'暂停':done?'再来一次':t.elapsed?'继续':'开始'} title={busy?'暂停':active.length?'开始此计时，并暂停当前计时':done?'再来一次':t.elapsed?'继续':'开始'} onClick={()=>toggle(t)}>{busy?<Pause size={14}/>:<Play size={14}/>}</button><button aria-label="重置计时" title="重置后可修改时长和模式" onClick={()=>{update(t.id,{elapsed:0,startedAt:null});setNow(Date.now());}}><RotateCcw size={13}/></button></div><div className="focus-timer-entry-options"><select aria-label={`计时 ${index+1} 模式`} disabled={locked} value={t.mode} onChange={e=>update(t.id,{mode:e.target.value as Entry['mode']})}><option value="down">倒计时</option><option value="up">正向计时</option></select>{t.mode==='down'&&!locked?<div className="focus-timer-hms">{[Math.floor(seconds/3600),Math.floor(seconds/60)%60,seconds%60].map((value,part)=><label key={part}><input aria-label={`计时 ${index+1} ${['小时','分钟','秒'][part]}`} type="number" min="0" max={part===0?999999:59} step="1" value={value} onChange={e=>{const n=Number(e.target.value);if(!Number.isInteger(n)||n<0||n>(part===0?999999:59))return;const units=[3600,60,1];update(t.id,{duration:(seconds+(n-value)*units[part])*1000});}}/>{['时','分','秒'][part]}</label>)}</div>:<span className="focus-timer-note" role="status">{done?'已完成':busy?'运行中':t.elapsed?'已暂停':'从零开始'}</span>}</div></article>;})}
    {!entries.length&&<p className="focus-timer-empty">点击右上角 + 添加计时</p>}</div>{entries.length>0&&<nav className="focus-timer-pages" aria-label="切换计时"><button aria-label="上一个计时" disabled={entries.length<2} onClick={()=>turn(-1)}><ChevronLeft size={14}/></button><div className="focus-timer-dots">{entries.map((t,index)=><button key={t.id} className={index===page?'selected':''} aria-label={`切换到 ${t.title.trim()||`计时 ${index+1}`}`} aria-current={index===page?'true':undefined} title={t.title.trim()||`计时 ${index+1}`} onClick={()=>{setSelected(index);setError('');}}><i/></button>)}</div><button aria-label="下一个计时" disabled={entries.length<2} onClick={()=>turn(1)}><ChevronRight size={14}/></button></nav>}{error&&<p className="focus-timer-error" role="alert">{error}</p>}</section>}
  </div>;
}
