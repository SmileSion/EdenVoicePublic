/** Local-only, bounded progress history. No network requests or video contents. */
export interface Progress { key: string; position: number; updated: number }
export const progressStorageKey = 'edenvoice.watch-progress.v1';
export function progressKey(server: string, channel: string, file: {name:string;size:number;duration:number}) {
  return JSON.stringify([server,channel,file.name,file.size,Math.round(file.duration)]);
}
export function readProgress(storage: Pick<Storage,'getItem'>): Progress[] {
  try { const value=JSON.parse(storage.getItem(progressStorageKey)||'[]');
    return Array.isArray(value)?value.filter(v=>typeof v?.key==='string'&&Number.isFinite(v.position)&&v.position>=0&&Number.isFinite(v.updated)).slice(0,100):[];
  } catch { return []; }
}
export function resumePosition(position:number|undefined,duration:number) {
  return position!==undefined&&Number.isFinite(position)&&position>=10&&position<duration-15?position:null;
}
export function writeProgress(storage: Pick<Storage,'getItem'|'setItem'>,key:string,position:number) {
  if(!Number.isFinite(position)||position<0)return;
  try { storage.setItem(progressStorageKey,JSON.stringify([{key,position,updated:Date.now()},...readProgress(storage).filter(v=>v.key!==key)].slice(0,100))); } catch { /* A full/disabled store must never interrupt watching. */ }
}
