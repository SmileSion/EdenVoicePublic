const modes={chat:{channel:'朋友的房间',sub:'4 位朋友 · 好久不见，今天聊什么？',eyebrow:'MAKE YOURSELF AT HOME',title:'今晚也在这里。',description:'不用做什么，待在一起就好。',bubble:'今晚，慢慢聊。',status:'语音在线',activity:'',caption:'进到熟悉的房间，朋友的头像、状态和聊天气泡，都在眼前。'},game:{channel:'朋友的房间',sub:'3 / 4 位朋友正在玩同一款游戏',eyebrow:'BETTER WITH YOUR SQUAD',title:'今晚一起玩 三角洲行动',description:'三角洲行动 · 熟悉的队友已经就位。',bubble:'人齐了，出发！',status:'三角洲行动',activity:'自动游戏模式',caption:'同房间过半成员玩同款游戏时，界面自动切换；识别目录由管理员维护。'},watch:{channel:'朋友的房间',sub:'共同观影 · 等待成员准备就绪',eyebrow:'TONIGHT, TOGETHER',title:'今晚，一起看点什么？',description:'各自的屏幕，同一刻的心动。',bubble:'准备好了！',status:'影片已就绪',activity:'主持人控制播放',caption:'从房间开启共同观影，成员准备本地影片。主持人和管理员控制全员播放及结束。'},screen:{channel:'朋友的房间',sub:'4 位朋友 · 一边说，一边看',caption:'屏幕共享就在当前房间中。成员主动加入观看，画面随窗口缩放，也能全屏查看。'}};
function selectMode(mode) {
  const m = modes[mode];
  const sharing = mode === 'screen';
  document.querySelectorAll('[data-mode]').forEach(b => b.setAttribute('aria-pressed', String(b.dataset.mode === mode)));
  document.querySelector('.room-scene').hidden = sharing;
  document.getElementById('share-demo').hidden = !sharing;
  for (const [id, value] of Object.entries({'room-name': m.channel, 'room-subtitle': m.sub, 'scene-caption': m.caption})) document.getElementById(id).textContent = value;
  if (sharing) return;
  document.querySelector('.room-scene').dataset.scene = mode;
  for (const [id, value] of Object.entries({'scene-eyebrow': m.eyebrow, 'scene-title': m.title, 'scene-description': m.description})) document.getElementById(id).textContent = value;
  document.querySelector('.bubble').textContent = m.bubble;
  document.querySelectorAll('.person small').forEach((el, i) => el.textContent = mode === 'game' && i === 0 ? '在线' : m.status);
  const activity = document.getElementById('scene-activity');
  activity.textContent = m.activity; activity.hidden = !m.activity;
}
document.getElementById('share-back').addEventListener('click', () => selectMode('chat'));
document.getElementById('share-fullscreen').addEventListener('click', async () => {
  const status = document.getElementById('share-demo-status');
  status.hidden = true;
  try { await document.getElementById('shared-canvas').requestFullscreen(); }
  catch { status.textContent = '当前浏览器暂不支持全屏预览。'; status.hidden = false; }
});
document.querySelectorAll('[data-memory-filter]').forEach(button => button.addEventListener('click', () => {
  const filter = button.dataset.memoryFilter;
  document.querySelectorAll('[data-memory-filter]').forEach(b => b.setAttribute('aria-pressed', String(b === button)));
  document.querySelectorAll('[data-memory-kind]').forEach(card => card.hidden = filter !== 'all' && card.dataset.memoryKind !== filter);
  const count = document.querySelectorAll('[data-memory-kind]:not([hidden])').length;
  document.getElementById('archive-demo-status').textContent = `${count} 段回忆 · 这里展示的是交互示意`;
}));
document.querySelectorAll('[data-mode]').forEach(b=>b.addEventListener('click',()=>selectMode(b.dataset.mode)));
const wave=document.querySelector('#wave-lines');for(let n=0;n<34;n++){const points=[];for(let x=0;x<=1440;x+=8){const e=Math.exp(-Math.pow((x-720)/480,2));points.push(`${x},${(150+Math.sin(x/130+n*.12)*e*(42+n*2.4)).toFixed(1)}`);}const path=document.createElementNS('http://www.w3.org/2000/svg','polyline');path.setAttribute('points',points.join(' '));path.setAttribute('opacity',String(.12+Math.sin(n/34*Math.PI)*.65));path.setAttribute('stroke-width','.9');wave.appendChild(path);}
let queued=false;function progress(){const distance=document.documentElement.scrollHeight-innerHeight;document.querySelector('.read-progress').style.transform=`scaleX(${distance>0?Math.min(1,Math.max(0,scrollY/distance)):0})`;queued=false;}addEventListener('scroll',()=>{if(!queued){queued=true;requestAnimationFrame(progress);}},{passive:true});addEventListener('resize',progress);progress();
// This is a local product demonstration. Timing uses elapsed wall time, not interval counts.
const timers=[{label:'留给自己的半小时',seconds:1800},{label:'一起读会儿书',seconds:0},{label:'休息五分钟',seconds:300}];let selected=0,running=false,elapsed=0,started=0,tick=null;
const toggle=document.getElementById('timer-toggle');function renderTimer(){const t=timers[selected];let spent=elapsed+(running?(Date.now()-started)/1000:0);if(t.seconds&&spent>=t.seconds){spent=t.seconds;elapsed=spent;running=false;clearInterval(tick);}const seconds=t.seconds?Math.max(0,Math.ceil(t.seconds-spent)):Math.floor(spent);document.getElementById('timer-digits').textContent=`${String(Math.floor(seconds/60)).padStart(2,'0')}:${String(seconds%60).padStart(2,'0')}`;toggle.textContent=running?'暂停':t.seconds&&spent>=t.seconds?'再来一次':elapsed?'继续':'开始专注';toggle.setAttribute('aria-label',`${toggle.textContent}演示计时`);}
toggle.addEventListener('click',()=>{if(running){elapsed+=(Date.now()-started)/1000;running=false;clearInterval(tick);}else{if(timers[selected].seconds&&elapsed>=timers[selected].seconds)elapsed=0;started=Date.now();running=true;tick=setInterval(renderTimer,250);}renderTimer();});function resetTimer(){running=false;elapsed=0;clearInterval(tick);renderTimer();}document.getElementById('timer-reset').addEventListener('click',resetTimer);document.querySelectorAll('[data-timer]').forEach(b=>b.addEventListener('click',()=>{selected=Number(b.dataset.timer);resetTimer();document.getElementById('timer-label').textContent=timers[selected].label;document.querySelectorAll('[data-timer]').forEach(dot=>dot.setAttribute('aria-pressed',String(dot===b)));}));addEventListener('pagehide',()=>clearInterval(tick));
const reducedMotion=matchMedia('(prefers-reduced-motion: reduce)');if('IntersectionObserver' in window&&!reducedMotion.matches){const observer=new IntersectionObserver(entries=>{for(const e of entries)if(e.isIntersecting){e.target.classList.add('arrived');observer.unobserve(e.target);}},{threshold:.08});document.querySelectorAll('.intro,.sound .wrap,.section-heading,.feature,.ownership-layout,.download-grid,.server-downloads').forEach(el=>{el.classList.add('reveal');observer.observe(el);});reducedMotion.addEventListener('change',e=>{if(e.matches){document.querySelectorAll('.reveal').forEach(el=>el.classList.add('arrived'));observer.disconnect();}});}
