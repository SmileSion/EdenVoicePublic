import { useEffect, useState, useMemo } from 'react';
import { Avatar } from './Avatar';
import type { CoreSnapshot } from './types';

export function FoyerAtmosphere({snapshot}:{snapshot:CoreSnapshot}) {
  const [hour,setHour]=useState(()=>new Date().getHours());
  useEffect(()=>{const refresh=()=>{if(!document.hidden)setHour(new Date().getHours());};const timer=window.setInterval(refresh,60000);document.addEventListener('visibilitychange',refresh);return()=>{clearInterval(timer);document.removeEventListener('visibilitychange',refresh);};},[]);
  const en=snapshot.language==='english';
  // Server-confirmed scenes only. Never infer another majority in the UI.
  const rooms=useMemo(()=>{
    const members=new Map<string,CoreSnapshot['users']>();
    for(const user of snapshot.users){if(!snapshot.roomScenes?.[user.channelId]?.game_id)continue;const list=members.get(user.channelId)||[];list.push(user);members.set(user.channelId,list);}
    return snapshot.channels.filter(c=>members.has(c.id)).map(c=>({channel:c,scene:snapshot.roomScenes![c.id],users:members.get(c.id)!}));
  },[snapshot.channels,snapshot.roomScenes,snapshot.users]);
  const signature=JSON.stringify(rooms.map(r=>[r.channel.id,r.channel.name,r.scene.title,r.users.map(u=>[u.sessionId,u.avatar,u.nickname])]));
  const [displayed,setDisplayed]=useState(rooms);
  const activeIDs=new Set(rooms.map(r=>r.channel.id));
  useEffect(()=>{
    setDisplayed(previous=>[...rooms,...previous.filter(r=>!activeIDs.has(r.channel.id))].slice(0,3));
    const timer=window.setTimeout(()=>setDisplayed(rooms.slice(0,3)),700);
    return()=>clearTimeout(timer);
  },[signature]);
  return <div className="foyer-atmosphere" data-time={hour>=7&&hour<17?'day':hour>=17&&hour<20?'dusk':'night'}>
    <div className="foyer-sky" aria-hidden="true"><div className="foyer-orb"/><div className="foyer-ridge far"/><div className="foyer-ridge near"/></div>
    <div className="foyer-window-copy"><small>WHERE WE GATHER</small><h3>{en?'A little closer, together':'相聚，不必匆忙'}</h3><p>{en?'Your friends are somewhere nearby.':'朋友们，就在不远处。'}</p></div>
    <div className="foyer-neighborhood" aria-label={en?'Friends’ game rooms':'朋友们的游戏窗景'}>
      {displayed.slice(0,3).map(r=><article aria-hidden={!activeIDs.has(r.channel.id)} className={`foyer-lit-window ${activeIDs.has(r.channel.id)?'':'leaving'}`} key={r.channel.id}><div className="foyer-window-glow" aria-hidden="true"/><small title={r.channel.name}>{r.channel.name}</small><strong title={r.scene.title}>{r.scene.title}</strong><div className="foyer-window-members">{r.users.slice(0,3).map(u=><Avatar key={u.sessionId} avatar={u.avatar} name={u.nickname} size={18} revision={snapshot.avatarCacheRevision} previewable={false}/>)}<span>{r.users.length} {en?'in room':'人在房间'}</span></div></article>)}
      {rooms.length>3&&<span className="foyer-more-lights">{en?`${rooms.length-3} more lights on`:`还有 ${rooms.length-3} 处灯亮着`}</span>}
    </div>
    <div className="foyer-rug" aria-hidden="true"/><div className="foyer-coffee-table" aria-hidden="true"><i/><b/></div><div className="foyer-plant" aria-hidden="true"><i/><i/><i/><b/></div>
    <p className="foyer-caption">{en?'Choose a room. Bring your voice closer.':'选个房间，声音就近了。'}</p>
  </div>;
}
