import assert from 'node:assert/strict';
import test from 'node:test';
import {roomKey,roomPreferences,updateRoom,preferences,updatePreferences,newerMessage,diagnosticAdvice} from '../src/experienceState.ts';
import {mockSnapshot} from '../src/mock.ts';

const storage=new Map<string,string>();
Object.assign(globalThis,{window:new EventTarget(),localStorage:{getItem:(key:string)=>storage.get(key)||null,setItem:(key:string,value:string)=>storage.set(key,value)}});

test('drafts, reading positions and notification overrides isolate server identity, client and room',()=>{
  storage.clear();
  const key=roomKey(mockSnapshot,'2');updateRoom(key,{draft:'房间草稿😀',read:'18446744073709551614',anchor:'4',muted:true});
  assert.equal(roomPreferences(key)?.draft,'房间草稿😀');
  for(const other of [roomKey(mockSnapshot,'3'),roomKey({...mockSnapshot,serverAddress:'other'},'2'),roomKey({...mockSnapshot,announcementScope:'new-identity'},'2'),roomKey({...mockSnapshot,clientId:'other'},'2')])assert.equal(roomPreferences(other),undefined);
  assert.equal(newerMessage('18446744073709551615',roomPreferences(key)?.read),true);
  assert.equal(newerMessage('18446744073709551614',roomPreferences(key)?.read),false);
  updateRoom(key,{draft:''});assert.equal(roomPreferences(key)?.draft,'');assert.equal(roomPreferences(key)?.muted,true);
  updatePreferences({dnd:true,sounds:false});assert.deepEqual(preferences(),{dnd:true,sounds:false});
});

test('local state is bounded, handles corrupt and unavailable storage',()=>{
  storage.clear();for(let i=0;i<130;i++)updateRoom('room'+i,{draft:'x'.repeat(2048)});
  assert.equal(Object.keys(JSON.parse(storage.get('eden.experience.v1')!).rooms).length,100);
  storage.set('eden.experience.v1','{broken');assert.equal(preferences().dnd,false);
  const original=localStorage.setItem;localStorage.setItem=()=>{throw Error('full')};assert.doesNotThrow(()=>updateRoom('x',{draft:'keep call alive'}));localStorage.setItem=original;
});

test('advice uses observed quality and device limits, no offline network diagnosis',()=>{
  const snapshot=structuredClone(mockSnapshot);snapshot.audio.inputSampleRate=16000;snapshot.diagnostics.effectiveAudioLossPercent=8;snapshot.diagnostics.rttMs=250;
  const titles=diagnosticAdvice(snapshot).map(tip=>tip.title);assert.ok(titles.includes('设备采样率较低'));assert.ok(titles.includes('网络可能影响通话'));
  snapshot.connection='disconnected';assert.ok(!diagnosticAdvice(snapshot).some(tip=>tip.action==='network'));
});
