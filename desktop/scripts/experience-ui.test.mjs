import assert from 'node:assert/strict';
import {createRequire} from 'node:module';
import {readFile} from 'node:fs/promises';
import {createServer} from 'node:http';
import {fileURLToPath} from 'node:url';
import {buildBrowserFixture,fixtureStyles} from './browser-fixture.mjs';
const root=fileURLToPath(new URL('..',import.meta.url));
const {chromium}=createRequire(import.meta.url)(process.env.PLAYWRIGHT_MODULE||'playwright');
const bundle=await buildBrowserFixture({stdin:{resolveDir:root,loader:'tsx',contents:`
import React from 'react';import{createRoot}from'react-dom/client';import{App}from'./src/App';import{mockSnapshot}from'./src/mock';
window.__TAURI_INTERNALS__={};window.calls=[];window.handlers={};window.sounds=[];
window.state=structuredClone(mockSnapshot);window.state.chatQuerySupported=true;window.state.chatHeads={'2':'103','3':'0'};window.state.savedServers=[{address:'other.example:7001',nickname:'test',displayName:'收藏服务器',serverName:''}];
window.state.messages=[101,102,103].map(id=>({id:String(id),channelId:'2',senderSessionId:'9002',senderClientId:'other',senderName:'Alice',senderAvatar:'L',text:'current '+id,sentAtUnixMs:1700000000000+id*1000}));
window.patch=next=>{window.state={...window.state,...next,revision:window.state.revision+1};for(const fn of window.handlers['core-state']||[])fn({payload:window.state});};
window.move=channel=>window.patch({users:window.state.users.map(user=>user.sessionId===window.state.selfSessionId?{...user,channelId:channel}:user)});
window.rpc=async(name,args)=>{
if(name==='get_snapshot')return window.state;
if(name==='get_close_preferences')return {behavior:'minimize',pending:false,trayAvailable:true};
if(name==='dispatch_core'){window.calls.push(args.command);return;}
if(name==='query_chat'){window.calls.push({type:'query_chat',...args.request});if(window.deferHistory)return new Promise(resolve=>window.resolveHistory=resolve);return {messages:[{id:'80',channelId:args.request.channel,senderClientId:'other',senderSessionId:'9002',senderName:'Old',senderAvatar:'L',text:args.request.query?'历史唯一关键词':'older page',sentAtUnixMs:1700000080000}],nextCursor:''};}
return null;};
createRoot(document.getElementById('root')).render(<React.StrictMode><App/></React.StrictMode>);
`},bundle:true,write:false,format:'iife',jsx:'automatic',loader:{'.svg':'dataurl'},plugins:[{name:'native',setup(b){
b.onResolve({filter:/uiSounds$/},a=>({path:a.path,namespace:'sound'}));b.onLoad({filter:/.*/,namespace:'sound'},()=>({contents:'export const playUiSound=async(name)=>window.sounds.push(name);export const prepareUiAudio=async()=>{};'}));
b.onResolve({filter:/^@tauri-apps\/api\/(core|event|window)$/},a=>({path:a.path,namespace:'mock'}));b.onLoad({filter:/.*/,namespace:'mock'},()=>({contents:'export const invoke=(...a)=>window.rpc(...a);export const listen=async(name,fn)=>{(window.handlers[name]??=[]).push(fn);return()=>window.handlers[name]=window.handlers[name].filter(f=>f!==fn);};export const getCurrentWindow=()=>({});'}));
}}]});
const css=(await Promise.all(['styles.css','watch-party.css','room-experience.css','close-behavior.css'].map(f=>readFile(root+'/src/'+f,'utf8')))).join('\n')+fixtureStyles(bundle);
const server=createServer((req,res)=>{res.setHeader('Content-Type',req.url==='/app.js'?'text/javascript':req.url==='/style.css'?'text/css':'text/html; charset=utf-8');res.end(req.url==='/app.js'?bundle.outputFiles[0].text:req.url==='/style.css'?css:'<html><meta charset="utf-8"><link rel="stylesheet" href="/style.css"><div id="root"></div><script src="/app.js"></script></html>');});
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
let browser;
try{
browser=await chromium.launch({executablePath:process.env.CHROME_EXECUTABLE});const page=await browser.newPage({viewport:{width:1000,height:760}});page.setDefaultTimeout(6000);
const errors=[];page.on('pageerror',e=>errors.push(String(e)));await page.goto('http://127.0.0.1:'+server.address().port);await page.locator('.workspace').waitFor();
await page.evaluate(()=>window.patch({chatHeads:{'2':'104','3':'0'},messages:[...window.state.messages,{id:'104',channelId:'2',senderClientId:'other',senderSessionId:'9002',senderName:'Alice',senderAvatar:'L',text:'unread arrival',sentAtUnixMs:1700000104000}]}));
await page.getByLabel('开黑小队 有未读消息').waitFor();assert.equal(await page.evaluate(()=>window.sounds.filter(x=>x==='message').length),1);
await page.getByRole('button',{name:'聊天',exact:true}).click();await page.getByLabel('开黑小队 有未读消息').waitFor({state:'detached'});
const input=page.locator('.composer input:not([type="file"])');await input.fill('草稿 A');await page.evaluate(()=>window.move('3'));assert.equal(await input.inputValue(),'');await input.fill('草稿 B');await page.evaluate(()=>window.move('2'));assert.equal(await input.inputValue(),'草稿 A');
await page.reload();await page.getByRole('button',{name:'聊天',exact:true}).click();assert.equal(await input.inputValue(),'草稿 A','draft survives restart');
await page.evaluate(()=>window.patch({messages:Array.from({length:100},(_,i)=>({id:String(101+i),channelId:'2',senderClientId:'other',senderSessionId:'9002',senderName:'Alice',senderAvatar:'L',text:'read position '+i,sentAtUnixMs:1700000000000+i*1000})),chatHeads:{'2':'200','3':'0'}}));
await page.waitForFunction(()=>document.querySelectorAll('[data-message-id]').length===100);
await page.locator('.messages').evaluate(node=>{node.scrollTop=600;node.dispatchEvent(new Event('scroll',{bubbles:true}));});
const anchor=()=>page.locator('.messages').evaluate(node=>{const top=node.getBoundingClientRect().top;return [...node.querySelectorAll('[data-message-id]')].find(row=>row.getBoundingClientRect().bottom>top)?.dataset.messageId;});
const savedAnchor=await anchor();await page.evaluate(()=>window.move('3'));await page.getByRole('heading',{name:'游戏交流',exact:true}).waitFor();await page.evaluate(()=>window.move('2'));
await page.waitForFunction(id=>{const node=document.querySelector('.messages'),top=node.getBoundingClientRect().top;return [...node.querySelectorAll('[data-message-id]')].find(row=>row.getBoundingClientRect().bottom>top)?.dataset.messageId===id;},savedAnchor);
await page.getByRole('button',{name:'加载更早消息',exact:true}).click();await page.getByText('older page',{exact:true}).waitFor();
assert.equal(await page.evaluate(()=>window.calls.find(c=>c.type==='query_chat').before),'101');
await page.getByRole('button',{name:'搜索当前频道消息',exact:true}).click();await page.getByPlaceholder('搜索当前频道消息').fill('唯一关键词');await page.getByPlaceholder('搜索当前频道消息').press('Enter');await page.getByText('历史唯一关键词',{exact:true}).waitFor();
await page.getByRole('button',{name:'关闭搜索',exact:true}).click();
await page.getByRole('button',{name:'搜索当前频道消息',exact:true}).click();await page.getByPlaceholder('搜索当前频道消息').fill('deferred');await page.evaluate(()=>window.deferHistory=true);await page.getByPlaceholder('搜索当前频道消息').press('Enter');await page.waitForFunction(()=>!!window.resolveHistory);await page.evaluate(()=>window.move('3'));
await page.evaluate(()=>{window.resolveHistory({messages:[{id:'79',channelId:'2',senderClientId:'other',senderSessionId:'9002',senderName:'Old',senderAvatar:'L',text:'STALE_HISTORY_LEAK',sentAtUnixMs:1}],nextCursor:''});window.deferHistory=false;});
assert.equal(await page.getByText('STALE_HISTORY_LEAK',{exact:true}).count(),0);await page.evaluate(()=>window.move('2'));
await page.getByLabel('通知设置').click();await page.getByLabel('免打扰（保留未读标记）').check();await page.getByLabel('静音当前房间通知').check();
const before=await page.evaluate(()=>window.sounds.length);await page.evaluate(()=>window.patch({chatHeads:{'2':'105','3':'0'}}));assert.equal(await page.evaluate(()=>window.sounds.length),before);
await page.getByLabel('通知设置').click();
await input.focus();await page.keyboard.press('Control+k');await page.getByRole('dialog',{name:'快捷跳转'}).waitFor();await page.getByPlaceholder('搜索服务器、房间或成员').fill('游戏交流');await page.keyboard.press('Enter');
assert.equal(await page.evaluate(()=>window.calls.filter(c=>c.type==='join_channel').at(-1).channelId),'3');
await page.keyboard.press('Control+k');await page.getByPlaceholder('搜索服务器、房间或成员').fill('收藏服务器');await page.keyboard.press('Enter');await page.getByRole('button',{name:'确认并连接'}).waitFor();assert.equal(await page.evaluate(()=>window.calls.filter(c=>c.type==='disconnect').length),0);await page.getByRole('button',{name:'取消',exact:true}).click();
await page.evaluate(()=>window.patch({audio:{...window.state.audio,selectedInput:'USB mic',activeInput:'Built-in mic',inputs:[]}}));
const restore=page.getByRole('button',{name:'恢复原设备',exact:true});await restore.waitFor();await page.waitForFunction(()=>[...document.querySelectorAll('button')].some(button=>button.textContent==='恢复原设备'&&button.disabled));assert.equal(await restore.isDisabled(),true);await page.evaluate(()=>window.patch({audio:{...window.state.audio,inputs:['USB mic']}}));await restore.click();assert.ok(await page.evaluate(()=>window.calls.some(c=>c.type==='retry_audio')));
await page.getByRole('button',{name:'关闭设备提示'}).click();
await page.evaluate(()=>window.patch({audio:{...window.state.audio,inputSampleRate:16000},diagnostics:{...window.state.diagnostics,rttMs:300,effectiveAudioLossPercent:8}}));
await page.getByRole('button',{name:'打开设置',exact:true}).click();await page.getByRole('button',{name:'语音诊断',exact:true}).click();await page.getByText('设备采样率较低',{exact:true}).waitFor();await page.getByRole('button',{name:'打开音频设置',exact:true}).click();await page.getByText('输入与输出',{exact:true}).waitFor();
await page.screenshot({path:'/tmp/edenvoice-experience-audio.png'});
assert.deepEqual(errors,[]);console.log('PASS: persistent scoped drafts, unread/read and DND, history/search, shortcut jump and server consent, fallback recovery, actionable diagnostics');
}finally{await browser?.close();await new Promise(resolve=>server.close(resolve));}
