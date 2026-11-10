import {test} from 'node:test';
import assert from 'node:assert/strict';
import {progressKey,readProgress,resumePosition,writeProgress} from '../src/watchProgress.ts';
test('bookmarks separate servers, channels and editions',()=>{const file={name:'movie',size:10,duration:100};const key=progressKey('a','1',file);assert.notEqual(key,progressKey('b','1',file));assert.notEqual(key,progressKey('a','2',file));assert.notEqual(key,progressKey('a','1',{...file,size:11}));});
test('finished and insignificant positions are not offered',()=>{for(const value of [undefined,0,9,NaN,Infinity,90,100])assert.equal(resumePosition(value,100),null);assert.equal(resumePosition(30,100),30);});
test('history is bounded and tolerates invalid/full storage',()=>{let data='bad';const store={getItem:()=>data,setItem:(_key:string,value:string)=>{data=value;}};assert.deepEqual(readProgress(store),[]);for(let i=0;i<120;i++)writeProgress(store,String(i),i);assert.equal(readProgress(store).length,100);writeProgress(store,'119',20);assert.equal(readProgress(store)[0].position,20);assert.equal(readProgress(store).length,100);assert.doesNotThrow(()=>writeProgress({...store,setItem:()=>{throw Error();}},'x',20));});
