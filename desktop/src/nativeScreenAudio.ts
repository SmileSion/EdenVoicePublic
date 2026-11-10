import type {ScreenContext} from './screenShareController';
import {invoke} from '@tauri-apps/api/core';
export type NativeScreenAudio = {track:MediaStreamTrack;stop:()=>void;pause:(paused:boolean)=>Promise<void>};
// Dedicated worklet output is connected only to WebRTC, never local speakers.
const processor=`class ScreenPCM extends AudioWorkletProcessor {
 constructor(){super();this.data=new Float32Array(19200);this.r=0;this.n=0;this.ready=false;this.paused=false;
 this.port.onmessage=({data})=>{if(typeof data?.paused==='boolean'){this.paused=data.paused;this.r=0;this.n=0;this.ready=false;this.data.fill(0);return;}if(this.paused)return;for(let i=0;i+1<data.length;i+=2){if(this.n===9600){this.r=(this.r+1)%9600;this.n--;}
 const p=(this.r+this.n)%9600;this.data[p*2]=data[i];this.data[p*2+1]=data[i+1];this.n++;}};}
 process(inputs,outputs){const out=outputs[0];if(!this.ready&&this.n>=sampleRate*.02)this.ready=true;
 for(let i=0;i<out[0].length;i++){if(this.paused||!this.ready||!this.n){this.ready=false;out[0][i]=0;out[1][i]=0;continue;}
 out[0][i]=this.data[this.r*2];out[1][i]=this.data[this.r*2+1];this.r=(this.r+1)%9600;this.n--;}return true;}
}registerProcessor('eden-screen-pcm',ScreenPCM);`;
export async function startNativeScreenAudio(captureContext:ScreenContext,onFailure:()=>void):Promise<NativeScreenAudio>{
 const token=crypto.randomUUID();let context:AudioContext|undefined,node:AudioWorkletNode|undefined;
 let paused=false,epoch=0;
 let closed=false,timer:ReturnType<typeof setTimeout>|undefined,track:MediaStreamTrack|undefined;
 const stop=()=>{if(closed)return;closed=true;clearTimeout(timer);track?.stop();node?.disconnect();if(context)void context.close().catch(()=>{});void invoke('stop_screen_audio',{token}).catch(()=>{});};
 try{
  const rate=await invoke<number>('start_screen_audio',{token,context:captureContext});
  context=new AudioContext({sampleRate:rate});
  const url=URL.createObjectURL(new Blob([processor],{type:'text/javascript'}));
  try{await context.audioWorklet.addModule(url);}finally{URL.revokeObjectURL(url);}
  node=new AudioWorkletNode(context,'eden-screen-pcm',{numberOfInputs:0,numberOfOutputs:1,outputChannelCount:[2]});
  const destination=context.createMediaStreamDestination();node.connect(destination);track=destination.stream.getAudioTracks()[0];
  let resumeTimer:ReturnType<typeof setTimeout>|undefined;
  try{await Promise.race([context.resume(),new Promise<never>((_,reject)=>{resumeTimer=setTimeout(()=>reject(new Error('系统音频处理器未能启动，请重新点击共享')),5000);})]);}finally{clearTimeout(resumeTimer);}
  const poll=async()=>{if(closed)return;try{
   const readEpoch=epoch;const samples=await invoke<number[]>('read_screen_audio',{token});if(closed)return;
   if(!paused&&readEpoch===epoch&&samples.length){const pcm=new Float32Array(samples);node!.port.postMessage(pcm,[pcm.buffer]);}
   timer=setTimeout(()=>void poll(),20);
  }catch{if(!closed){stop();onFailure();}}};
  const pause=async(value:boolean)=>{
   if(closed)throw new Error('系统音频采集已停止');
   epoch++;paused=true;track!.enabled=false;node!.port.postMessage({paused:true});
   await invoke('set_screen_audio_paused',{token,paused:value});
   if(closed)throw new Error('系统音频采集已停止');
   epoch++;paused=value;node!.port.postMessage({paused:value});
  };
  void poll();return {track,stop,pause};
 }catch(error){stop();throw error;}
}
