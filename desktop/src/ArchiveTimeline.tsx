import { useMemo, useState, type CSSProperties } from 'react';
import { Clock3, Gamepad2, ImagePlus, Search, Users } from 'lucide-react';
import { Avatar } from './Avatar';
import type { ArchiveEvent, RoomArchive } from './types';

type Props = {archive: RoomArchive; english?: boolean; copy: (event: ArchiveEvent) => string[]; onRecap: (event: ArchiveEvent) => void};
export function ArchiveTimeline({archive, english=false, copy, onRecap}: Props) {
  const [kind,setKind]=useState('all'), [game,setGame]=useState(''), [search,setSearch]=useState('');
  const [expanded,setExpanded]=useState<string[]>([]);
  const games=useMemo(()=>[...new Map(archive.events.filter(e=>e.game).map(e=>[e.game!.game_id,e.game!.title])).entries()],[archive.events]);
  const visible=archive.events.filter(event=>{
    if(kind==='evening'&&!event.evening)return false;
    if(kind==='game'&&!event.game || kind==='room'&&event.game || game&&event.game?.game_id!==game)return false;
    const names=[...new Set([...event.triggeredBy,...event.participants])].map(id=>archive.participantProfiles[id]?.displayName||'').join(' ');
    return [...copy(event),event.game?.title||'',names].join(' ').toLocaleLowerCase().includes(search.trim().toLocaleLowerCase());
  }).slice().sort((a,b)=>Date.parse(b.occurredAt)-Date.parse(a.occurredAt));
  const locale=english?'en-US':'zh-CN';
  const day=(event:ArchiveEvent)=>new Date(event.occurredAt).toLocaleDateString(locale,{month:'long',day:'numeric',year:'numeric'});
  const time=(stamp:string)=>new Date(stamp).toLocaleTimeString(locale,{hour:'2-digit',minute:'2-digit',hour12:false});
  return <div className="memory-browser">
    <div className="memory-filters"><div className="memory-tabs" aria-label={english?'Memory type':'回忆类型'}>{[['all',english?'All':'全部'],['evening',english?'Evenings':'这一晚'],['game',english?'Together in game':'一起玩'],['room',english?'Room moments':'房间时刻']].map(([key,label])=><button key={key} aria-pressed={kind===key} onClick={()=>{setKind(key);if(key!=='game')setGame('');}}>{label}</button>)}</div>
      <label className="memory-search"><Search size={13}/><input aria-label={english?'Search memories':'搜索回忆'} value={search} onChange={e=>setSearch(e.target.value)} placeholder={english?'Game, friend or memory':'找游戏、朋友或某段回忆'}/></label>
      {kind==='game'&&<select aria-label={english?'Filter game':'筛选游戏'} value={game} onChange={e=>setGame(e.target.value)}><option value="">{english?'All games':'所有游戏'}</option>{games.map(([id,title])=><option key={id} value={id}>{title}</option>)}</select>}
    </div>
    <div className="memory-results" role="status">{english?`${visible.length} memories in this view`:`这一页，留下了 ${visible.length} 段回忆`}{archive.hasMore&&(english?' · Recent records only':' · 筛选范围为已加载的最近记录')}</div>
    <div className="memory-scroll">
      {!visible.length&&<div className="memory-empty"><Gamepad2 size={26}/><h4>{english?'A little quiet here':'这里暂时安静'}</h4><p>{search||game? (english?'Try another game or friend.':'换个游戏或朋友的名字试试。'):(english?'Shared moments will appear here.':'一起度过的时光，会慢慢留在这里。')}</p><button onClick={()=>{setKind('all');setGame('');setSearch('');}}>{english?'Show all':'看看全部回忆'}</button></div>}
      {visible.map((event,index)=>{
        const [title,description]=copy(event), memory=event.game;
        const ids=[...new Set([...event.participants,...event.triggeredBy])];
        const isExpanded=expanded.includes(event.id), people=isExpanded?ids:ids.slice(0,5);
        const minutes=Math.floor(Math.max(0,memory?.duration_seconds||0)/60);
        const duration=minutes>=60?`${Math.floor(minutes/60)}${english?'h':' 小时'} ${minutes%60}${english?'m':' 分'}`:`${minutes}${english?'m':' 分钟'}`;
        const hue=memory?[...memory.game_id].reduce((n,c)=>n+c.charCodeAt(0),0)%90+190:270;
        return <section className="memory-group" key={event.id}>
          {(index===0||day(visible[index-1])!==day(event))&&<h4 className="memory-date">{day(event)}<span/></h4>}
          <div className="memory-row"><time>{time(event.occurredAt)}</time><article className={`memory-card ${memory?'is-game':'is-room'}`} style={{'--memory-hue':hue} as CSSProperties}>
            <div className="memory-card-top"><span>{memory?<Gamepad2 size={13}/>:<Users size={13}/>} {event.evening?(english?'AN EVENING TOGETHER':'这一晚 · 相聚回顾'):memory?(english?'PLAYED TOGETHER':'一起玩的时光'):(english?'ROOM MOMENT':'房间的一个瞬间')}</span><button aria-label={english?`Make card: ${title}`:`制作回顾卡片：${title}`} title={english?'Make a keepsake':'制作回顾卡片'} onClick={()=>onRecap(event)}><ImagePlus size={15}/></button></div>
            <h3>{memory?.title||title}</h3>{event.evening&&<p>{[...event.evening.games,...event.evening.movies].join(' · ')|| (english?'Time spent together':'一起聊天，也值得记住。')}</p>}
            {memory?<><p className="memory-game-note">{english?'The game ends. The company stays.':'游戏会结束，一起的时光会留下。'}</p><div className="memory-session"><span><Clock3 size={12}/>{duration}</span><span>{time(memory.started_at)} — {time(memory.ended_at)}</span><span>{ids.length} {english?'friends':'位伙伴'}</span></div></>:<p>{description}</p>}
            {!!ids.length&&<div className="memory-people"><div className="memory-avatars">{people.map(id=>{const profile=archive.participantProfiles[id];const name=profile?.displayName||(english?'A friend':'一位朋友');return <span key={id} title={name}><Avatar avatar={profile?.avatar||''} name={name} size={29}/>{isExpanded&&<small>{name}</small>}</span>;})}</div><div className="memory-names">{ids.slice(0,3).map(id=>archive.participantProfiles[id]?.displayName||(english?'A friend':'一位朋友')).join('、')}{ids.length>3&&(english?' and friends':'等朋友')}</div>{ids.length>5&&<button aria-expanded={isExpanded} onClick={()=>setExpanded(isExpanded?expanded.filter(id=>id!==event.id):[...expanded,event.id])}>{isExpanded?(english?'Less':'收起'):`+${ids.length-5}`}</button>}</div>}
          </article></div>
        </section>;
      })}
    </div>
  </div>;
}

export function ArchiveJournal({archive,english=false,copy,onRecap,onOverview,close,all,onAll}:{archive:RoomArchive;english?:boolean;copy:Props['copy'];onRecap:Props['onRecap'];onOverview:()=>void;close:()=>void;all?:boolean;onAll?:()=>void}) {
  return <section className="archive-page archive-journal" onMouseDown={e=>e.stopPropagation()}>
    <header><div><small>EDEN ARCHIVE / {archive.stats.roomName}</small><h2>{english?'Some evenings deserve another page.':'有些夜晚，值得再翻一页。'}</h2><p>{english?'Remember the company, and the time you shared.':'记住一起玩的人，也记住一起度过的时间。'}</p></div><button aria-label={english?'Close archive':'关闭档案'} onClick={close}>×</button></header>
    <div className="archive-summary"><span><small>{english?'Days together':'创建至今'}</small><strong>{archive.ageDays} {english?'days':'天'}</strong></span><span><small>{english?'Active days':'活跃相聚'}</small><strong>{archive.stats.activeDays} {english?'days':'天'}</strong></span><span><small>{english?'Voice time':'有效语音'}</small><strong>{Math.floor(archive.stats.activeVoiceSeconds/3600)} {english?'hours':'小时'}</strong></span><span><small>{english?'Friends':'曾来过的人'}</small><strong>{archive.stats.historicalMembers}</strong></span><span><small>{english?'Longest gathering':'最长会话'}</small><strong>{Math.floor(archive.stats.longestSessionSeconds/60)} {english?'min':'分钟'}</strong></span></div>
    <div className="archive-timeline-heading"><div><h3>Memory Timeline</h3><p>{english?'The moments you shared, at your own pace.':'不记录输赢，只留下「我们一起」。'}</p></div><nav className="archive-timeline-actions"><button className="archive-create-card" onClick={onOverview}>{english?'Make a keepsake':'制作回顾卡片'}</button>{onAll&&<button onClick={onAll}>{all?(english?'Highlights':'只看重要记录'):(english?'All traces':'全部记录')}</button>}</nav></div>
    <ArchiveTimeline archive={archive} english={english} copy={copy} onRecap={onRecap}/>
  </section>;
}
