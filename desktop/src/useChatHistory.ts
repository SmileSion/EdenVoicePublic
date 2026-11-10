import {useEffect,useRef,useState} from 'react';
import {compareMessages} from './experienceState';
import {invoke} from '@tauri-apps/api/core';
import type {CoreSnapshot,ChatMessage} from './types';
type Page={messages:ChatMessage[];nextCursor:string};
export function useChatHistory(snapshot:CoreSnapshot,channel:string,query:string){
  const key=JSON.stringify([snapshot.serverAddress,snapshot.announcementScope,snapshot.selfSessionId,snapshot.connection,channel,query]);
  const current=useRef(key);current.current=key;
  const [state,setState]=useState<{key:string;messages:ChatMessage[];cursor:string|null;busy:boolean;error:string}>({key,messages:[],cursor:null,busy:false,error:''});
  const active=state.key===key?state:{key,messages:[],cursor:null,busy:false,error:''};
  const pending=useRef('');
  const generation=useRef(0);
  useEffect(()=>{generation.current++;pending.current='';setState({key,messages:[],cursor:null,busy:false,error:''});},[key]);
  const load=async()=>{
    if(pending.current===key||!snapshot.chatQuerySupported||snapshot.connection!=='connected'||!channel||active.cursor==='')return;
    const token=++generation.current;
    pending.current=key;setState(previous=>({...active,...(previous.key===key?previous:{}),key,busy:true,error:''}));
    try {
      const result=await invoke<Page>('query_chat',{request:{server:snapshot.serverAddress,fingerprint:snapshot.announcementScope||'',session:snapshot.selfSessionId,channel,before:query?'':snapshot.messages.filter(message=>message.channelId===channel).at(0)?.id||'',cursor:active.cursor||'',query}});
      if(current.current!==key||token!==generation.current)return;
      setState(previous=>{
        const merged=new Map((previous.key===key?previous.messages:[]).map(message=>[message.id,message]));
        for(const message of result.messages)if(message.channelId===channel)merged.set(message.id,message);
        return {key,messages:[...merged.values()].sort(compareMessages).slice(-1000),cursor:merged.size>=1000?'':result.nextCursor,busy:false,error:''};
      });
    }catch(error){if(current.current===key&&token===generation.current)setState(previous=>({...previous,key,busy:false,error:String(error)}));}
    finally{if(token===generation.current&&pending.current===key)pending.current='';}
  };
  return {...active,load,supported:!!snapshot.chatQuerySupported};
}
