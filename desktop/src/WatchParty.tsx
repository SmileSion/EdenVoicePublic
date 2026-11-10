import { FoyerAtmosphere } from "./FoyerAtmosphere";
import { useEffect, useMemo, useRef, useState } from "react";
import { Film, Gamepad2, Play, Pause, FolderOpen, RotateCcw, Users, Settings2, Crown, Check, MicOff, MoreHorizontal, UserRound } from "lucide-react";
import { Avatar } from "./Avatar";
import { RoomSocialActions, RoomAwayNotice, RoomAwayBadge, SeatReaction, SideRoomReactions, roomPresence, usePresenceClock, type RoomSocialView } from "./RoomPresence";
import { dispatch } from "./bridge";
import { useWatchRoom } from "./useWatchRoom";
import { gameActivityLabel, seatPreview, hasVoiceChannel } from "./room-presentation";
import type { CoreSnapshot, User, ChatMessage } from "./types";

const clock=(seconds:number)=>{const n=Math.max(0,Math.floor(seconds));return `${Math.floor(n/3600).toString().padStart(2,"0")}:${Math.floor(n%3600/60).toString().padStart(2,"0")}:${(n%60).toString().padStart(2,"0")}`;};
const PAGE_SIZE=20;
type Confirm="open"|"leave"|"close"|"play"|null;
export function RoomExperience({snapshot,channelId,visible,onReveal,onSettings,onReadMessage,social,onAway,onReaction}:{snapshot:CoreSnapshot;channelId:string;visible:boolean;onReveal:()=>void;onSettings?:()=>void;onReadMessage?:(id:string)=>void;social?:RoomSocialView;onAway?:(active:boolean)=>void;onReaction?:(emoji:string)=>void}) {
  const {view,error,busy,available,run,choose,reopen,roomCommand,resume,restore,dismissResume}=useWatchRoom(snapshot,channelId);
  const gameScene=snapshot.roomScenes?.[channelId];
  const appearance=gameScene?.game_id?'game':'lounge';
  const [confirm,setConfirm]=useState<Confirm>(null),[menu,setMenu]=useState(false),[transfer,setTransfer]=useState("");
  const [query,setQuery]=useState(""),[page,setPage]=useState(0),[showMembers,setShowMembers]=useState(false),[seek,setSeek]=useState<number|null>(null);
  const stageRef=useRef<HTMLDivElement>(null);
  const memberListRef=useRef<HTMLElement>(null);
  useEffect(()=>{if(showMembers)memberListRef.current?.scrollIntoView({block:"nearest"});},[showMembers]);
  const [stageWidth,setStageWidth]=useState(640),[stageHeight,setStageHeight]=useState(500);
  const [seatColumns,setSeatColumns]=useState(7),[seatRows,setSeatRows]=useState(1);
  const foyer=!hasVoiceChannel(channelId);
  const [messageClock,setMessageClock]=useState(Date.now);
  const messageNow=Math.max(messageClock,Date.now());
  const senderCounts=new Map<string,number>();
  const roomMessages=snapshot.messages.filter(message=>!!channelId&&message.channelId===channelId&&message.sentAtUnixMs<=messageNow&&messageNow-message.sentAtUnixMs<10000).reverse().filter(message=>{
    const sender=message.senderClientId||message.senderSessionId||message.senderName;
    const count=senderCounts.get(sender)||0;senderCounts.set(sender,count+1);return count<2;
  }).slice(0,6).reverse();
  const nextMessageBoundary=roomMessages.reduce((next,message)=>Math.min(next,message.sentAtUnixMs+(messageNow-message.sentAtUnixMs<8500?8500:10000)),Infinity);
  useEffect(()=>{if(!Number.isFinite(nextMessageBoundary))return;const timer=window.setTimeout(()=>setMessageClock(Date.now()),Math.max(1,nextMessageBoundary-Date.now()));return()=>clearTimeout(timer);},[nextMessageBoundary]);
  useEffect(()=>{const el=stageRef.current;if(!el)return;const measure=()=>{const width=el.clientWidth;setStageHeight(el.clientHeight); if(width>0)setStageWidth(width); if(width>0)setSeatColumns(Math.max(3,Math.min(12,Math.floor((width-40)/80))));setSeatRows(window.innerHeight>=720?2:1);};const observer=new ResizeObserver(measure);observer.observe(el);window.addEventListener("resize",measure);measure();return()=>{observer.disconnect();window.removeEventListener("resize",measure);};},[]);
  const dialogRef=useRef<HTMLDialogElement>(null), menuRef=useRef<HTMLDivElement>(null);
  const en=snapshot.language==="english",t=(zh:string,english:string)=>en?english:zh;
  const users=useMemo(()=>snapshot.users.filter(u=>foyer||u.channelId===channelId),[snapshot.users,channelId]);
  const byId=useMemo(()=>new Map(users.map(u=>[u.sessionId,u])),[users]);
  const presenceNow=usePresenceClock(social);
  const presence=roomPresence(social,users.map(user=>user.sessionId),presenceNow,!foyer);
  const ownAway=presence.away[snapshot.selfSessionId];
  const canSocial=!foyer&&social?.supported&&users.some(user=>user.sessionId===snapshot.selfSessionId);
  const reactionsBySession=new Map(presence.reactions.map(reaction=>[reaction.sessionId,reaction]));
  const members=useMemo(()=>new Map(view.members.map(m=>[m.sessionId,m])),[view.members]);
  const host=view.hostId===snapshot.selfSessionId;
  const hostFile=members.get(view.hostId)?.file;
  const ownFile=members.get(snapshot.selfSessionId)?.file;
  const film=hostFile?.name||view.selectedFile||ownFile?.name;
  const mismatch=!!ownFile&&view.members.some(m=>m.file&&(m.file.size!==ownFile.size||Math.abs(m.file.duration-ownFile.duration)>1));
  const unready=view.members.filter(m=>!m.ready).length;
  const seatedUsers=view.roomOpen?users.filter(u=>members.has(u.sessionId)):users;
  const totalSeats=view.roomOpen?view.members.length:users.length;
  const belongsTo=(message:ChatMessage,user:User)=>message.senderSessionId===user.sessionId||(!message.senderSessionId&&message.senderName===user.nickname);
  const initialCapacity=seatColumns*seatRows;
  const initialSeats=seatPreview(seatedUsers,snapshot.selfSessionId,view.hostId,totalSeats>initialCapacity?initialCapacity-1:initialCapacity);
  const sideNeeded=roomMessages.some(message=>!initialSeats.some(user=>belongsTo(message,user)))||presence.reactions.some(reaction=>!initialSeats.some(user=>user.sessionId===reaction.sessionId));
  const layoutColumns=foyer?Math.max(3,Math.min(10,Math.floor(stageWidth*.7/90))):sideNeeded?Math.max(1,Math.floor((stageWidth-216)/80)):seatColumns;
  const capacity=layoutColumns*(foyer?(stageHeight<420?1:2):seatRows);
  const seats=seatPreview(seatedUsers,snapshot.selfSessionId,view.hostId,totalSeats>capacity?Math.max(0,capacity-1):capacity);
  const foyerSeatCount=seats.length+(totalSeats>seats.length?1:0)+(foyer&&totalSeats<=3?Math.min(2,3-totalSeats):0);
  const foyerSeatStyle=(index:number)=>{
    if(!foyer)return undefined;
    const backCount=stageHeight<420?foyerSeatCount:Math.ceil(foyerSeatCount/2),front=index>=backCount;
    const rowCount=front?foyerSeatCount-backCount:backCount;
    const t=rowCount<=1?.5:(index-(front?backCount:0))/(rowCount-1);
    const curve=1-Math.pow(2*t-1,2);
    return {left:`${15+t*70}%`,top:`${front?65+curve*12:12-curve*12}%`} as import('react').CSSProperties;
  };
  const sideReactions=presence.reactions.filter(reaction=>!seats.some(user=>user.sessionId===reaction.sessionId));
  const sideMessages=roomMessages.filter(message=>!seats.some(user=>belongsTo(message,user)));
  const bubble=(message:ChatMessage,withName=false)=>{
    const truncated=message.text.length>48||message.text.split('\n').length>2;
    const preview=message.text.slice(0,48).split('\n').slice(0,2).join('\n');
    return <article className={messageNow-message.sentAtUnixMs>=8500?'fading':''} key={message.id} data-room-message-id={message.id}>{withName&&<strong>{message.senderName}</strong>}<p>{truncated?preview+'…':message.text}</p>{truncated&&<button onClick={()=>onReadMessage?.(message.id)}>{t('查看完整','View full message')}</button>}</article>;
  };
  const filtered=users.filter(u=>`${u.nickname} ${u.sessionId}`.toLocaleLowerCase().includes(query.toLocaleLowerCase()));
  const pages=Math.max(1,Math.ceil(filtered.length/PAGE_SIZE));const currentPage=Math.min(page,pages-1);
  const targets=users.filter(u=>members.has(u.sessionId)&&u.sessionId!==view.hostId);
  const disabled=busy||view.managing||view.loading||!available;
  useEffect(()=>{if(!view.canClose&&confirm==='close')setConfirm(null);if((!view.canControl||!view.roomOpen)&&confirm==='play')setConfirm(null);if(!view.active&&confirm==='leave')setConfirm(null);if(view.roomOpen&&confirm==='open')setConfirm(null);if(!view.canTransferHost)setTransfer("");if(!view.roomOpen){setMenu(false);setSeek(null);}},[view.canClose,view.canControl,view.active,view.roomOpen,view.canTransferHost,confirm]);
  useEffect(()=>{
    const el=dialogRef.current;if(!el)return;
    if(confirm&&!el.open)el.showModal();else if(!confirm&&el.open)el.close();
  },[confirm]);
  useEffect(()=>{if(!visible){setMenu(false);setConfirm(null);}},[visible]);
  useEffect(()=>{if(!menu)return;const close=(e:PointerEvent)=>{if(!menuRef.current?.contains(e.target as Node))setMenu(false);};const key=(e:KeyboardEvent)=>{if(e.key==='Escape')setMenu(false);};document.addEventListener('pointerdown',close);document.addEventListener('keydown',key);return()=>{document.removeEventListener('pointerdown',close);document.removeEventListener('keydown',key);};},[menu]);
  const control=(action:"play"|"pause"|"seek"|"sync",position?:number)=>run(()=>dispatch({type:"watch_control",action,position}));
  const play=()=>{if(view.roomPaused&&(unready>0||mismatch))setConfirm('play');else void control(view.roomPaused?'play':'pause');};
  const status=view.roomOpen?(view.roomPaused?t('准备 / 暂停','Preparing / paused'):t('同步播放中','Playing together')):foyer?t('朋友在线','Friends online'):appearance==='game'?t('一起开黑','Playing together'):t('自由聊天','Voice lounge');
  const confirmAction=()=>{
    const action=confirm;setConfirm(null);
    if(action==='open')void run(()=>roomCommand('open'));
    if(action==='close'&&view.canClose)void run(()=>roomCommand('close'));
    if(action==='leave'&&view.active)void run(()=>dispatch({type:'watch_leave'}));
    if(action==='play'&&view.canControl)void control('play');
  };
  const stateLabel=(user:User)=>{
    if(foyer){const channel=snapshot.channels.find(c=>c.id===user.channelId);return channel?`${user.speaking?t('正在说话','Speaking'):t('在','In')} · ${channel.name}`:hasVoiceChannel(user.channelId)?t('语音中','In voice'):t('在线 · 未加入语音','Online · outside voice');}
    if(presence.away[user.sessionId])return t('暂离 · 已静音','Away · muted');
    const member=members.get(user.sessionId);
    if(view.roomOpen)return member?(member.ready?t('已就绪','Ready'):t('准备影片中','Preparing video')):t('仅语音','Voice only');
    if(appearance==='game')return gameActivityLabel(user.gameActivity)||t('未分享游戏状态','No shared game activity');
    return user.muted||user.adminMuted?t('已静音','Muted'):user.speaking?t('正在说话','Speaking'):t('语音在线','In voice');
  };
  return <section className={`room-experience ${foyer?'foyer-experience':''} ${visible?'expanded':'collapsed'}`} data-theme={snapshot.theme} aria-label={t('朋友的房间','Friends room')}>
    {!visible&&<div className="room-watch-strip"><Film size={14}/><span>{view.roomOpen?`${t('共同观影','Watch together')} · ${status} · ${view.members.length} ${t('人','members')}`:t('朋友们在房间里','Friends are in the room')}</span><button onClick={onReveal}>{view.roomOpen?t('返回放映室','Back to cinema'):t('查看房间','View room')}</button></div>}
    <div className="room-content" hidden={!visible}>
      <div className="room-toolbar"><span className="room-status"><i/>{status}<small>{users.length} {foyer?t('人在线','online'):t('人语音','in voice')}{view.roomOpen?` · ${view.members.length} ${t('人观影','watching')}`:''}</small></span>
        {visible&&canSocial&&social&&<RoomSocialActions key={channelId} social={social} ownAway={ownAway} english={en} onAway={onAway} onReaction={onReaction}/>}
        {view.roomOpen?<div ref={menuRef} className="room-menu-anchor"><button aria-label={t('观影操作','Watch actions')} aria-expanded={menu} onClick={()=>setMenu(!menu)}><MoreHorizontal size={17}/></button>{menu&&<div className="room-menu"><button onClick={()=>{setMenu(false);onSettings?.();}}><Settings2 size={13}/>{t('播放器设置','Player settings')}</button>{view.active&&<button disabled={disabled} onClick={()=>{setMenu(false);setConfirm('leave');}}>{t('退出观影','Leave watch room')}</button>}{view.canClose&&<button className="danger" disabled={disabled} onClick={()=>{setMenu(false);setConfirm('close');}}>{t('结束全员观影','End watch room')}</button>}</div>}</div>:null}
      </div>
      {canSocial&&ownAway&&<RoomAwayNotice english={en}/>}
      {canSocial&&social?.error&&<p className="room-social-error" role="alert">{social.error}</p>}
      <div ref={stageRef} data-density={stageHeight<420?"compact":"normal"} data-rows={seats.length+(totalSeats>seats.length?1:0)>layoutColumns?2:1} style={{"--seat-columns":Math.min(layoutColumns,totalSeats||1),"--speech-lane":sideNeeded?"176px":"0px"} as import("react").CSSProperties} data-population={totalSeats<=3?'quiet':'social'} className={`friend-stage ${foyer?'foyer-stage':''} ${view.roomOpen?'cinema':appearance} ${view.roomOpen&&!view.roomPaused?'playing':''}`} aria-label={t('房间场景','Room scene')}>
        <div className="friend-stage-label">{view.roomOpen?"EDEN PRIVATE CINEMA":foyer?"WHERE WE GATHER":appearance==='game'?"AFTER HOURS · GAME NIGHT":"OUR LITTLE CORNER"}<span>{view.roomOpen?status:t('和朋友待在一起','A place to be together')}</span></div><div className="friend-curtain left"/><div className="friend-curtain right"/><div className="friend-light"/><div className="friend-floor"/><div className="friend-wall-art"/><div className="friend-lamp"/>
        <div className="friend-screen"><div className="friend-mountain back"/><div className="friend-mountain front"/><div className="friend-screen-copy"><small>{view.roomOpen?'TONIGHT, TOGETHER':appearance==='game'?'ONE MORE ROUND':'MAKE YOURSELF AT HOME'}</small><h3 title={film}>{view.roomOpen?(film||t('今晚，一起看点什么？','What shall we watch?')):appearance==='game'?t(`今晚一起玩 ${gameScene?.title||''}`,`Playing ${gameScene?.title||''} together`):foyer?t('朋友们都在这里','Your friends are here'):t('今晚也在这里','Together again tonight')}</h3><p>{view.roomOpen?(view.active?t('各自准备影片，一起同步观看','Local videos, shared playback'):t('加入观影，或留在语音里陪伴','Join the watch room or stay in voice')):appearance==='game'?t('朋友已在房间，等你一起开局','Your friends are here, ready to play'):t('不用做什么，待在一起就好','Nothing to do. Just be here.')}</p></div></div>

        {(!!sideMessages.length||!!sideReactions.length)&&<aside className="room-side-activity" aria-label={t("其他成员的动态","Other members’ activity")}>{!!sideReactions.length&&<SideRoomReactions reactions={sideReactions} names={new Map(users.map(user=>[user.sessionId,user.nickname]))} now={presenceNow} english={en}/>} {!!sideMessages.length&&<div className="side-messages" aria-label={t("其他成员消息","Other members’ messages")}>{sideMessages.map(message=>bubble(message,true))}</div>}</aside>}
        {!view.roomOpen&&appearance==='game'&&<div className="game-equipment" aria-hidden="true"><div className="game-tower"><i/><i/></div><div className="game-desk"/><div className="game-keyboard"/><Gamepad2 className="game-controller" size={32}/></div>}{foyer&&<FoyerAtmosphere snapshot={snapshot}/>}<div className="friend-platform"/><div className="friend-seats">{seats.map((user,index)=><div style={foyerSeatStyle(index)} className={`friend-seat ${members.get(user.sessionId)?.ready?'ready':''}`} key={user.sessionId} data-seat-session={user.sessionId} data-away={!!presence.away[user.sessionId]}>{roomMessages.some(message=>belongsTo(message,user))&&<div className="seat-messages" aria-label={`${user.nickname} 的消息`}>{roomMessages.filter(message=>belongsTo(message,user)).map(message=>bubble(message))}</div>}<Avatar avatar={user.avatar} name={user.nickname} size={34} speaking={user.speaking&&hasVoiceChannel(user.channelId)} previewable={false} revision={snapshot.avatarCacheRevision} fallback={<UserRound size={18}/>}/>{presence.away[user.sessionId]&&<RoomAwayBadge english={en}/>} {reactionsBySession.has(user.sessionId)&&<SeatReaction key={reactionsBySession.get(user.sessionId)!.id} reaction={reactionsBySession.get(user.sessionId)!} nickname={user.nickname} now={presenceNow} english={en}/>}<span title={user.nickname}>{user.nickname}{user.sessionId===view.hostId&&view.roomOpen&&<Crown size={10}/>}</span><small title={stateLabel(user)}>{stateLabel(user)}</small></div>)}{totalSeats>seats.length&&<button style={foyerSeatStyle(seats.length)} className="friend-seat overflow" onClick={()=>{setShowMembers(true);setPage(0);}} aria-label={t('查看全部房间成员','View all room members')}><Users size={18}/><strong>+{totalSeats-seats.length}</strong><span>{t('查看成员','Members')}</span></button>}{foyer&&totalSeats<=3&&Array.from({length:Math.min(2,3-totalSeats)},(_,i)=><div style={foyerSeatStyle(seats.length+i)} className="friend-seat empty-seat" key={'empty-'+i} aria-hidden="true"/>)}{!foyer&&totalSeats===0&&<p className="friend-empty">{foyer?t('客厅还很安静，等朋友们上线。','A quiet lounge, waiting for friends.'):t('开启后，邀请朋友一起加入。','Open the room and invite your friends.')}</p>}</div>
      </div>
      <div className="room-action-area">
        {!available&&!foyer&&<p className="room-notice">{t('加入支持共同观影的语音频道后即可开启。','Join a supported voice channel to start watching.')}</p>}
        {view.roomOpen&&<div className="room-film-summary"><span><Film size={13}/><strong title={film}>{film||t('等待选择影片','Waiting for a video')}</strong></span><small>{t('就绪','Ready')} {view.members.length-unready} / {view.members.length}</small></div>}
        {view.active&&view.ready&&<div className="room-timeline"><span>{clock(seek??view.position)}</span><input type="range" aria-label={t('全员播放进度','Global playback position')} min={0} max={Math.max(1,view.duration)} step={1} disabled={disabled||!view.canControl} value={seek??Math.min(view.position,view.duration)} onChange={e=>setSeek(Number(e.target.value))} onPointerUp={e=>{if(view.canControl){void control('seek',Number(e.currentTarget.value));setSeek(null);}}} onKeyUp={e=>{if(view.canControl&&['ArrowLeft','ArrowRight','Home','End','PageUp','PageDown'].includes(e.key)){void control('seek',Number(e.currentTarget.value));setSeek(null);}}}/><span>{clock(view.duration)}</span></div>}
        {resume!==null&&<div className="watch-resume" role="status"><span>上次看到 {Math.floor(resume/3600)>0?`${Math.floor(resume/3600)}:`:""}{String(Math.floor(resume/60)%60).padStart(2,"0")}:{String(Math.floor(resume)%60).padStart(2,"0")}</span><button disabled={disabled} onClick={()=>void restore()}>恢复全员进度</button><button onClick={dismissResume}>暂不恢复</button></div>}
        <div className="room-actions">
          {!view.active&&!foyer&&<button className="primary" disabled={disabled} onClick={()=>view.roomOpen?void run(()=>roomCommand('join')):setConfirm('open')}><Film size={14}/>{view.roomOpen?t('加入观影','Join watch room'):t('开启共同观影','Start watch room')}</button>}
          {view.active&&<>{view.canReopen&&<button disabled={disabled||!view.mpvPath} onClick={()=>void reopen()}><RotateCcw size={13}/>{t('重新打开影片','Reopen video')}</button>}<button disabled={disabled||!view.mpvPath} onClick={()=>void choose()}><FolderOpen size={14}/>{view.selectedFile?t('更换影片','Change video'):t('选择本地影片','Choose local video')}</button>{view.ready&&<><button className="primary" disabled={disabled||!view.canControl} onClick={play}>{view.roomPaused?<Play size={14}/>:<Pause size={14}/>} {view.roomPaused?t('一起播放','Play together'):t('暂停全员','Pause everyone')}</button><button disabled={disabled} onClick={()=>void control('sync')}><RotateCcw size={13}/>{t('同步自己','Sync myself')}</button></>}</>}
          <button className="room-members-toggle" aria-expanded={showMembers} onClick={()=>setShowMembers(!showMembers)}><Users size={13}/>{t('成员','Members')} {users.length}</button>
        </div>
        {view.active&&!view.mpvPath&&<div className="room-notice">{t('首次观影需要配置 mpv。','Configure mpv before your first video.')}<button onClick={onSettings}>{t('播放器设置','Player settings')}</button></div>}
        {view.active&&<p className="room-caption">{view.loading?t('正在加载影片…','Loading video…'):view.status} {view.canControl?t('你可以控制全员播放。','You can control global playback.'):t('播放由主持人和管理员控制。','Playback is controlled by the host and admins.')}</p>}
        {view.managing&&<p role="status" className="room-caption">{t('正在等待服务器确认…','Waiting for server confirmation…')}</p>}
        {mismatch&&<p className="room-notice">{t('影片大小或时长不同，请确认大家使用相同版本。','Video sizes or durations differ. Check your versions.')}</p>}
        {(error||view.error)&&<p className="room-error" role="alert">{error||view.error}</p>}
      </div>
      {showMembers&&<section ref={memberListRef} className="room-member-browser" aria-label={t('房间成员列表','Room member list')}><header>{foyer&&<button onClick={()=>setShowMembers(false)} aria-label={t('关闭成员列表','Close members')}>×</button>}<strong>{t('房间成员','Room members')} · {users.length}</strong><input aria-label={t('搜索房间成员','Search room members')} placeholder={t('搜索昵称','Search names')} value={query} onChange={e=>{setQuery(e.target.value);setPage(0);}}/></header><div className="room-member-rows">{filtered.slice(currentPage*PAGE_SIZE,(currentPage+1)*PAGE_SIZE).map(user=><div className="room-member-item" key={user.sessionId}><Avatar avatar={user.avatar} name={user.nickname} size={28} revision={snapshot.avatarCacheRevision} previewable={false}/>{presence.away[user.sessionId]&&<RoomAwayBadge english={en}/>}<div><strong title={user.nickname}>{user.nickname}{user.sessionId===snapshot.selfSessionId?` · ${t('你','you')}`:''}</strong><small title={stateLabel(user)}>{stateLabel(user)}</small></div>{user.muted||user.adminMuted?<MicOff size={12}/>:members.get(user.sessionId)?.ready?<Check size={12}/>:null}{foyer&&hasVoiceChannel(user.channelId)&&<button onClick={()=>void dispatch({type:'join_channel',channelId:user.channelId})}>{t('加入房间','Join room')}</button>}{view.canTransferHost&&members.has(user.sessionId)&&user.sessionId!==view.hostId&&<button disabled={disabled} onClick={()=>setTransfer(user.sessionId)}>{t('转交','Make host')}</button>}</div>)}</div>{!filtered.length&&<p>{t('没有匹配成员','No matching members')}</p>}<footer><button disabled={currentPage===0} onClick={()=>setPage(currentPage-1)}>{t('上一页','Previous')}</button><span>{currentPage+1} / {pages}</span><button disabled={currentPage+1>=pages} onClick={()=>setPage(currentPage+1)}>{t('下一页','Next')}</button></footer></section>}
      {transfer&&view.canTransferHost&&targets.some(u=>u.sessionId===transfer)&&<div className="room-notice">{t('将主持人转交给','Transfer hosting to')} {byId.get(transfer)?.nickname}<button disabled={disabled} onClick={()=>void run(async()=>{await roomCommand('transfer_host',transfer);setTransfer('');})}>{t('确认转交','Confirm transfer')}</button><button onClick={()=>setTransfer('')}>{t('取消','Cancel')}</button></div>}
      {!foyer&&<details className="room-help"><summary>{t('观影说明与成员权限','How watching and permissions work')}</summary><p>{t('每个人使用 mpv 播放自己的本地影片，文件不会上传。主持人和有权限的管理员控制全员播放；普通成员可以加入、选片和同步自己。未加入的人继续语音。切换到聊天不退出观影；退出只影响自己，最后一人退出时观影结束。主持人退出后自动转交主持权，结束全员观影需要确认。房间会根据大家正在玩的游戏自动切换外观；结束观影后恢复当前场景。','Each person plays a local video with mpv; files are not uploaded. Hosts and authorized admins control playback. Viewers can join, select a video and sync themselves. Chat view keeps watching active. Leaving only affects you; the last viewer ends the room. Hosting transfers when the host leaves. Ending everyone’s session requires confirmation. Room appearance follows shared game activity and resumes after watching.')}</p><button onClick={onSettings}><Settings2 size={13}/>{t('播放器设置','Player settings')}</button></details>}
    </div>
    <dialog ref={dialogRef} className="room-confirm" onCancel={()=>setConfirm(null)} onClick={e=>{if(e.target===e.currentTarget)setConfirm(null);}}><h3>{confirm==='open'?t('开启共同观影？','Start watching?'):confirm==='close'?t('结束全员观影？','End watching for everyone?'):confirm==='leave'?t('退出观影？','Leave watching?'):t('部分成员尚未准备好','Some members are not ready')}</h3><p>{confirm==='open'?t('你将成为主持人。其他成员自行加入，不会自动打开播放器。','You will host. Others choose whether to join; players do not open automatically.'):confirm==='close'?t('所有成员退出观影，恢复日常场景，语音通话继续。','Everyone leaves watching. The daily scene returns and voice continues.'):confirm==='leave'?(host?t('你退出后主持权自动转交；最后一人退出则结束观影。语音继续。','Hosting transfers when you leave; the last viewer ends watching. Voice continues.'):t('只退出自己的观影，语音通话继续。','Only you leave watching. Voice continues.')):t('部分成员未就绪或影片版本不同。可以等待大家，或仍然开始播放。','Some members are not ready or have different videos. Wait or play anyway.')}</p><footer><button onClick={()=>setConfirm(null)}>{t('取消','Cancel')}</button><button className={confirm==='close'?'danger':'primary'} disabled={busy} onClick={confirmAction}>{confirm==='open'?t('确认开启','Start watch room'):confirm==='close'?t('确认结束观影','End watch room'):confirm==='leave'?t('确认退出','Leave watch room'):t('仍然播放','Play anyway')}</button></footer></dialog>
  </section>;
}
