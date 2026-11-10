export type TimerState = { mode:'up'|'down'; duration:number; elapsed:number; startedAt:number|null };
export const timerDefault:TimerState={mode:'down',duration:30*60_000,elapsed:0,startedAt:null};
export const timerKey='eden.focus-timer.v1';
export function elapsedAt(state:TimerState,now=Date.now()){return state.elapsed+(state.startedAt===null?0:Math.max(0,now-state.startedAt));}
export function timerValue(state:TimerState,now=Date.now()){const elapsed=elapsedAt(state,now);return state.mode==='up'?elapsed:Math.max(0,state.duration-elapsed);}
export function timerDone(state:TimerState,now=Date.now()){return state.mode==='down'&&elapsedAt(state,now)>=state.duration;}
export function formatTimer(ms:number){const seconds=Math.max(0,Math.floor(ms/1000)),days=Math.floor(seconds/86400),hours=Math.floor(seconds/3600)%24,minutes=Math.floor(seconds/60)%60;return `${days?days+'天 ':''}${String(hours).padStart(2,'0')}:${String(minutes).padStart(2,'0')}:${String(seconds%60).padStart(2,'0')}`;}
export function parseTimer(value:string|null):TimerState{try{const s=JSON.parse(value||'null');if(s&&(s.mode==='up'||s.mode==='down')&&Number.isSafeInteger(s.duration)&&s.duration>0&&s.duration<=Number.MAX_SAFE_INTEGER/2&&Number.isFinite(s.elapsed)&&s.elapsed>=0&&(s.startedAt===null||Number.isFinite(s.startedAt)&&s.startedAt>=0))return s;}catch{/* Ignore damaged local preferences. */}return {...timerDefault};}
