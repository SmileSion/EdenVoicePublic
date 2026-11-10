import {useEffect,useRef,useState} from 'react';
import type {CoreSnapshot} from './types';
import {newerMessage,preferences,roomKey,roomPreferences,updateRoom} from './experienceState';
import {playUiSound} from './uiSounds';

export function useUnreadRooms(snapshot:CoreSnapshot){
  const [,render]=useState(0),previous=useRef<{scope:string;heads:Record<string,string>}|null>(null);
  useEffect(()=>{const change=()=>render(x=>x+1);window.addEventListener('eden-experience-changed',change);return()=>window.removeEventListener('eden-experience-changed',change);},[]);
  const scope=roomKey(snapshot,'');
  const heads={...snapshot.chatHeads};
  for(const message of snapshot.messages)if(newerMessage(message.id,heads[message.channelId]))heads[message.channelId]=message.id;
  const serialized=JSON.stringify(heads);
  useEffect(()=>{
    if(snapshot.connection!=='connected')return;
    const old=previous.current;previous.current={scope,heads};
    let notify=false;
    for(const [channel,head] of Object.entries(heads)){
      const key=roomKey(snapshot,channel),room=roomPreferences(key);
      if(!room?.read){updateRoom(key,{read:head});continue;}
      const own=snapshot.messages.some(message=>message.id===head&&message.senderClientId===snapshot.clientId);
      if(!own&&old?.scope===scope&&old.heads[channel]&&newerMessage(head,old.heads[channel])&&!room.muted&&newerMessage(head,room.read))notify=true;
    }
    const prefs=preferences();
    if(notify&&!prefs.dnd&&prefs.sounds)void playUiSound('message',snapshot.audio.outputVolume);
  },[scope,serialized,snapshot.connection]);
  return Object.fromEntries(Object.entries(heads).map(([channel,head])=>[channel,!!roomPreferences(roomKey(snapshot,channel))?.read&&newerMessage(head,roomPreferences(roomKey(snapshot,channel))?.read)]));
}
