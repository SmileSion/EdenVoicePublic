import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { ArchiveEvent, RoomArchive } from './types';

export function ArchiveRecap({archive,copy,close,initialEventId=''}:{initialEventId?:string;archive:RoomArchive;copy:(event:ArchiveEvent)=>string[];close:()=>void}) {
  const [selected,setSelected]=useState(initialEventId||archive.events.find(e=>e.evening)?.id||'');
  const [showRoom,setShowRoom]=useState(false);
  const [showNames,setShowNames]=useState(true);
  const [showActivities,setShowActivities]=useState(true);
  const [image,setImage]=useState('');
  const [status,setStatus]=useState('');
  const [saving,setSaving]=useState(false);
  const dialog=useRef<HTMLDialogElement>(null);
  const event=archive.events.find(value=>value.id===selected);
  const evening=event?.evening;
  const memoryEvents=event?[event]:archive.events;
  const names=[...new Set(memoryEvents.flatMap(item=>[...(item.triggeredBy||[]),...(item.participants||[])]))]
    .map(id=>archive.participantProfiles?.[id]?.displayName?.trim()).filter((name):name is string=>!!name);
  useEffect(()=>{dialog.current?.showModal();},[]);
  useEffect(()=>{
    const canvas=document.createElement('canvas');canvas.width=1200;canvas.height=1500;
    const ctx=canvas.getContext('2d');if(!ctx)return;
    const gradient=ctx.createLinearGradient(0,0,1200,1500);gradient.addColorStop(0,'#292a40');gradient.addColorStop(.65,'#181a2b');gradient.addColorStop(1,'#343047');
    ctx.fillStyle=gradient;ctx.fillRect(0,0,1200,1500);
    // An engraved arch and restrained light keep the keepsake close to the room aesthetic.
    const glow=ctx.createRadialGradient(955,240,10,955,240,570);
    glow.addColorStop(0,'#ddbb7822');glow.addColorStop(1,'#ddbb7800');ctx.fillStyle=glow;ctx.fillRect(0,0,1200,1000);
    ctx.strokeStyle='#c9b99855';ctx.lineWidth=1.5;
    ctx.beginPath();ctx.roundRect(62,62,1076,1376,[480,480,24,24]);ctx.stroke();
    ctx.strokeStyle='#bcb0d01c';ctx.beginPath();ctx.roundRect(79,79,1042,1342,[464,464,14,14]);ctx.stroke();
    ctx.fillStyle='#e5ccaa';ctx.beginPath();ctx.arc(976,330,33,0,Math.PI*2);ctx.fill();
    ctx.strokeStyle='#c4b6d013';for(let i=0;i<5;i++){ctx.beginPath();ctx.ellipse(590,1430,650+i*35,190+i*28,0,0,Math.PI*2);ctx.stroke();}
    const text=(value:string,x:number,y:number,size:number,color='#eee9e3',font='sans-serif')=>{ctx.font=`${size}px ${font}`;ctx.fillStyle=color;ctx.fillText(value,x,y);};
    const wrap=(value:string,y:number,size:number,maxLines:number,font='sans-serif')=>{
      ctx.font=`${size}px ${font}`;const chars=Array.from(value.replace(/\s+/g,' '));let line='',row=0;
      for(let i=0;i<chars.length;i++){
        if(ctx.measureText(line+chars[i]).width>930){text(line,132,y+row*size*1.5,size,'#eee9e3',font);row++;line='';}
        if(row===maxLines-1&&ctx.measureText(line+chars[i]+'…').width>930){text(line+'…',132,y+row*size*1.5,size,'#eee9e3',font);return;}
        line+=chars[i];
      }
      text(line,132,y+row*size*1.5,size,'#eee9e3',font);
    };
    text('E D E N   A R C H I V E',132,264,24,'#c6bbd2');
    const [title,description]=event?copy(event):['相聚，留下了回响。','那些一起度过的日子，都在这里。'];
    const date=event?new Date(event.occurredAt).toLocaleDateString('zh-CN'):`第 ${archive.ageDays} 天`;
    text(date,132,358,27,'#c6bbd2');
    wrap(title,464,68,2,'"Songti SC", "SimSun", serif');
    ctx.strokeStyle='#d3bb8880';ctx.beginPath();ctx.moveTo(132,638);ctx.lineTo(198,638);ctx.stroke();
    const activities=evening&&showActivities?[...evening.games.map(t=>'一起玩 · '+t),...evening.movies.map(t=>'一起看 · '+t)]:[];
    wrap(evening?'朋友们在这里，让平凡的一晚有了回响。':description,704,31,2);
    if(activities.length)wrap(activities.slice(0,2).join('  /  ')+(activities.length>2?`  · 还有 ${activities.length-2} 项活动`:''),817,25,2);
    if(showNames&&names.length){
      text(event?'一同留下这段回忆':'出现在这些回忆里',132,906,22,'#b8aec7');
      let x=132,y=930;
      names.slice(0,6).forEach((name,index)=>{
        if(index===3){x=132;y+=61;}
        ctx.font='26px sans-serif';let label='';
        for(const char of Array.from(name)){if(ctx.measureText(label+char).width>220){label+='…';break;}label+=char;}
        const width=ctx.measureText(label).width+38;
        ctx.fillStyle='#c5b5df10';ctx.strokeStyle='#c5b5df30';ctx.beginPath();ctx.roundRect(x,y,width,46,23);ctx.fill();ctx.stroke();
        text(label,x+19,y+32,26,'#ddd3e8');x+=width+14;
      });
      if(names.length>6)text(`另有 ${names.length-6} 位朋友`,132,1080,21,'#b8aec7');
    }
    text(evening?'这次相聚 · 已结束':'房间累计 · 截至本次回顾',132,1140,21,'#b8aec7');
    const stats=evening?[[`${Math.floor(evening.duration_seconds/3600)} 小时 ${Math.floor(evening.duration_seconds%3600/60)} 分`,'相聚时长'],[`${event!.participants.length} 位`,'一同相聚'],[new Date(evening.started_at).toLocaleTimeString('zh-CN',{hour:'2-digit',minute:'2-digit',hour12:false}),'开始于']]:[[`${archive.stats.activeDays} 天`,'活跃相聚'],[`${Math.floor(archive.stats.activeVoiceSeconds/3600)} 小时 ${Math.floor(archive.stats.activeVoiceSeconds%3600/60)} 分`,'有效语音'],[`${archive.stats.historicalMembers} 位`,'曾来过的人']];
    stats.forEach(([value,label],i)=>{text(value,132+i*320,1202,30);text(label,132+i*320,1246,23,'#b8aec7');});
    if(showRoom)wrap(archive.stats.roomName,1310,28,1);
    text('EdenVoice · 让相聚发生',132,1385,24,'#b8aec7');
    setImage(canvas.toDataURL('image/png'));setStatus('');
  },[archive,selected,showRoom,showNames,showActivities]);
  const save=async()=>{if(!image||saving)return;setSaving(true);try{
    if('__TAURI_INTERNALS__' in window){const result=await invoke<boolean>('export_archive_card',{image:image.split(',')[1]});setStatus(result?'回顾卡片已保存':'已取消保存');}
    else {const link=document.createElement('a');link.href=image;link.download='Eden-Archive.png';link.click();setStatus('回顾卡片已下载');}
  }catch{setStatus('保存失败，请重试。');}finally{setSaving(false);}};
  return <dialog ref={dialog} className="archive-recap" aria-labelledby="recap-title" onCancel={close}><header><div><small>留住一刻</small><h2 id="recap-title">回顾卡片</h2></div><button aria-label="关闭回顾卡片" onClick={close}>×</button></header><div className="recap-layout"><img src={image} alt="Eden Archive 回顾卡片预览"/><div className="recap-options"><label>选择回忆<select value={selected} onChange={e=>setSelected(e.target.value)}><option value="">房间相聚回顾</option>{archive.events.map(item=><option key={item.id} value={item.id}>{copy(item)[0]} · {new Date(item.occurredAt).toLocaleDateString()}</option>)}</select></label><label className="recap-check"><input type="checkbox" checked={showActivities} onChange={e=>setShowActivities(e.target.checked)}/>显示游戏和观影名称</label><label className="recap-check"><input type="checkbox" checked={showRoom} onChange={e=>setShowRoom(e.target.checked)}/>显示房间名称</label><label className="recap-check"><input type="checkbox" checked={showNames} disabled={!names.length} onChange={e=>setShowNames(e.target.checked)}/>显示成员昵称</label><p>{names.length?'昵称来自所选回忆，有昵称才会显示。':'这段回忆暂时没有可展示的昵称。'} 卡片不含服务器地址，保存后可自行分享。相聚卡片在房间结束后生成；累计概览不代表单晚时长。</p><button className="primary" disabled={saving||!image} onClick={()=>void save()}>{saving?'正在保存…':'保存为图片'}</button><span role="status">{status}</span></div></div></dialog>;
}
