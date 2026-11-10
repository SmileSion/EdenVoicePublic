import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { dispatch } from "./bridge";
import { progressKey, readProgress, resumePosition, writeProgress } from "./watchProgress";
import type { CoreSnapshot } from "./types";
export interface WatchFile { name: string; size: number; duration: number }
export interface WatchView {
  managing: boolean; canControl: boolean; canClose: boolean; canTransferHost: boolean; role: string;
  canReopen: boolean; active: boolean; loading: boolean; ready: boolean; roomOpen: boolean;
  channelId: string; hostId: string; mpvPath: string; selectedFile: string; error: string; status: string;
  position: number; paused: boolean; roomPaused: boolean; duration: number;
  members: { sessionId: string; ready: boolean; file?: WatchFile | null }[];
}
export const emptyWatch: WatchView = {managing:false,canControl:false,canClose:false,canTransferHost:false,role:"",canReopen:false,active:false,loading:false,ready:false,roomOpen:false,channelId:"",hostId:"",mpvPath:"",selectedFile:"",error:"",status:"",position:0,paused:true,roomPaused:true,duration:0,members:[]};
export function useWatchRoom(snapshot: CoreSnapshot, channelId: string) {
  const [view, setView] = useState(emptyWatch), [error,setError] = useState("");
  const [busy,setBusy] = useState(false);
  const context = JSON.stringify([snapshot.serverAddress, snapshot.selfSessionId, channelId, snapshot.connection]);
  const current = useRef(context); current.current = context;
  const mounted = useRef(true), inFlight = useRef(false);
  useEffect(()=>{mounted.current=true;return()=>{mounted.current=false;};},[]);
  const available = snapshot.connection === "connected" && snapshot.watchSupported && !!channelId;
  const roomCommand = (action: "status"|"open"|"join"|"close"|"transfer_host", targetSessionId?:string) => dispatch({type:"watch_room",action,targetSessionId,channelId,sessionId:snapshot.selfSessionId,serverAddress:snapshot.serverAddress});
  useEffect(()=>{
    setView(emptyWatch);setError("");setBusy(false);inFlight.current=false;
    if(!available || !("__TAURI_INTERNALS__" in window))return;
    let disposed=false,received=false;let unlisten:(()=>void)|undefined;
    const apply=(value:WatchView)=>{
      if(disposed||current.current!==context)return;
      setView(value.channelId&&value.channelId!==channelId ? emptyWatch : {...value,roomPaused:value.roomPaused??value.paused});
    };
    void(async()=>{
      unlisten=await listen<WatchView>("watch-state",e=>{received=true;apply(e.payload);});
      if(disposed){unlisten();return;}
      const value=await invoke<WatchView>("get_watch_snapshot");if(!received)apply(value);
    })().catch(e=>{if(!disposed)setError(String(e));});
    // One controller survives room/chat switching; hidden app tabs do not poll.
    let pending=false;
    const refresh=async()=>{if(disposed||document.hidden||pending)return;pending=true;try{await roomCommand("status");}catch(e){if(!disposed)setError(String(e));}finally{pending=false;}};
    void refresh();const timer=window.setInterval(()=>void refresh(),3000);
    const visible=()=>{if(!document.hidden)void refresh();};document.addEventListener("visibilitychange",visible);
    return()=>{disposed=true;unlisten?.();clearInterval(timer);document.removeEventListener("visibilitychange",visible);};
  },[context,available]);
  const run=async(action:()=>Promise<void>)=>{
    if(inFlight.current)return;inFlight.current=true;setBusy(true);setError("");
    try{await action();}catch(e){if(mounted.current&&current.current===context)setError(String(e));}
    finally{if(mounted.current&&current.current===context){inFlight.current=false;setBusy(false);}}
  };
  const choose=()=>run(async()=>{
    const file=await invoke<string|null>("select_watch_file");
    if(file&&mounted.current&&current.current===context)await dispatch({type:"watch_open",file,channelId,sessionId:snapshot.selfSessionId,serverAddress:snapshot.serverAddress});
  });
  const reopen=()=>run(()=>dispatch({type:"watch_reopen",channelId,sessionId:snapshot.selfSessionId,serverAddress:snapshot.serverAddress}));
  const [resume,setResume]=useState<number|null>(null);
  const progressSession=useRef("");
  const latestProgress=useRef<{key:string;position:number}|null>(null);
  const ownFile=view.members.find(member=>member.sessionId===snapshot.selfSessionId)?.file;
  const key=ownFile?progressKey(snapshot.serverAddress,channelId,ownFile):"";
  const host=view.hostId===snapshot.selfSessionId;
  const eligible=available&&view.active&&view.ready&&!view.loading&&!!key;
  useEffect(()=>{
    if(!eligible){progressSession.current="";setResume(null);return;}
    const session=context+key;
    if(progressSession.current!==session){
      progressSession.current=session;
      // A running session always wins over a local bookmark.
      setResume(host&&view.roomPaused&&view.position<5?resumePosition(readProgress(localStorage).find(v=>v.key===key)?.position,view.duration):null);
    }
  },[eligible,context,key,host]);
  useEffect(()=>{if(!host||!view.roomPaused)setResume(null);},[host,view.roomPaused]);
  useEffect(()=>{
    latestProgress.current=eligible&&resume===null?{key,position:view.position}:null;
  },[eligible,key,view.position,resume]);
  useEffect(()=>{
    const save=()=>{const value=latestProgress.current;if(value)writeProgress(localStorage,value.key,value.position);};
    const timer=window.setInterval(save,5000);
    window.addEventListener('pagehide',save);
    return()=>{save();clearInterval(timer);window.removeEventListener('pagehide',save);};
  },[context,key,eligible]);
  const restore=()=>run(async()=>{
    if(resume===null||!eligible||!host||!view.canControl||!view.roomPaused||current.current!==context)return;
    await dispatch({type:"watch_control",action:"seek",position:resume});
    setResume(null);
  });
  return {resume,restore,dismissResume:()=>setResume(null),view:available?view:emptyWatch,error,busy,available,run,choose,reopen,roomCommand};
}
