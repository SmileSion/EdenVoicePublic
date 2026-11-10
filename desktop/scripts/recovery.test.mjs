import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { fileURLToPath } from 'node:url';
import { buildBrowserFixture, fixtureStyles } from './browser-fixture.mjs';

const root = fileURLToPath(new URL('..', import.meta.url));
const { chromium } = createRequire(import.meta.url)(process.env.PLAYWRIGHT_MODULE || 'playwright');
const bundle = await buildBrowserFixture({
  stdin: { resolveDir: root, loader: 'tsx', contents: `
    import React from 'react';import {createRoot} from 'react-dom/client';
    import {App} from './src/App';import {mockSnapshot} from './src/mock';
    window.__TAURI_INTERNALS__={};window.calls=[];window.handlers={};window.reads=0;
    window.failBoot=location.search.includes('bootfail');window.rejectSend=false;
    window.state=structuredClone(mockSnapshot);
    Object.assign(window.state.shortcuts,{pushToTalk:'F8',toggleMic:'F7'});
    window.patch=next=>{window.state={...window.state,...next,revision:window.state.revision+1};
      for(const fn of window.handlers['core-state']||[])fn({payload:window.state});};
    window.rpc=async(name,args)=>{
      if(name==='get_snapshot'){window.reads++;if(window.failBoot)throw Error('test startup failure');return window.state;}
      if(name==='get_close_preferences')return {behavior:'minimize',pending:false,trayAvailable:true};
      if(name==='dispatch_core'){window.calls.push(args.command);if(window.rejectSend&&args.command.type==='send_text_message')throw Error('test queue full');}
      return null;
    };
    createRoot(document.getElementById('root')).render(<React.StrictMode><App/></React.StrictMode>);
  ` },
  bundle: true, write: false, format: 'iife', jsx: 'automatic', loader: { '.svg': 'text' },
  plugins: [{ name: 'native', setup(b) {
    b.onResolve({ filter: /^@tauri-apps\/api\/(core|event|window)$/ }, a => ({ path: a.path, namespace: 'mock' }));
    b.onLoad({ filter: /.*/, namespace: 'mock' }, () => ({ contents: `
      export const invoke=(...a)=>window.rpc(...a);
      export const listen=async(name,fn)=>{(window.handlers[name]??=[]).push(fn);return()=>{window.handlers[name]=window.handlers[name].filter(f=>f!==fn);};};
      export const getCurrentWindow=()=>({});
    ` }));
  } }],
});
const css = (await Promise.all(['styles.css','watch-party.css','room-experience.css','close-behavior.css'].map(f => readFile(root+'/src/'+f,'utf8')))).join('\n') + fixtureStyles(bundle);
const server = createServer((req,res) => {
  res.setHeader('Content-Type',req.url==='/app.js'?'text/javascript':req.url==='/style.css'?'text/css':'text/html; charset=utf-8');
  res.end(req.url==='/app.js'?bundle.outputFiles[0].text:req.url==='/style.css'?css:'<html><meta charset="utf-8"><link rel="stylesheet" href="/style.css"><div id="root"></div><script src="/app.js"></script></html>');
});
await new Promise(resolve => server.listen(0,'127.0.0.1',resolve));
let browser;
try {
  browser = await chromium.launch({ executablePath: process.env.CHROME_EXECUTABLE });
  const page = await browser.newPage({ viewport: { width: 1000, height: 760 } });
  const errors=[];page.on('pageerror',error=>errors.push(String(error)));
  const url='http://127.0.0.1:'+server.address().port;
  await page.goto(url);await page.locator('.workspace').waitFor();
  const calls=type=>page.evaluate(type=>window.calls.filter(c=>c.type===type),type);
  const patch=next=>page.evaluate(next=>window.patch(next),next);
  await page.keyboard.down('F8');await page.keyboard.press('KeyA');
  assert.equal((await calls('set_push_to_talk_pressed')).at(-1).pressed,true,'unrelated keyup must not release PTT');
  await page.evaluate(()=>window.dispatchEvent(new Event('blur')));
  assert.equal((await calls('set_push_to_talk_pressed')).at(-1).pressed,false,'blur releases PTT');
  await page.keyboard.up('F8');
  await page.keyboard.down('F8');
  await page.evaluate(()=>{Object.defineProperty(document,'hidden',{configurable:true,value:true});document.dispatchEvent(new Event('visibilitychange'));delete document.hidden;});
  assert.equal((await calls('set_push_to_talk_pressed')).at(-1).pressed,false,'hidden releases PTT');
  await page.keyboard.up('F8');
  await patch({shortcuts:{...await page.evaluate(()=>window.state.shortcuts),pushToTalk:'Control+F8'}});
  await page.keyboard.down('Control');await page.keyboard.down('F8');await page.keyboard.up('Control');
  assert.equal((await calls('set_push_to_talk_pressed')).at(-1).pressed,false,'modifier release ends chord');
  await page.keyboard.up('F8');
  await page.keyboard.down('F7');await page.keyboard.down('F7');await page.keyboard.up('F7');
  assert.equal((await calls('set_muted')).length,1,'auto repeat cannot repeatedly toggle mic');

  await page.getByRole('button',{name:'聊天',exact:true}).click();
  const input=page.locator('.composer input:not([type="file"])');
  const send=page.getByRole('button',{name:'发送消息',exact:true});
  await input.fill('保留这条草稿');await patch({connection:'reconnecting'});
  assert.equal(await send.isDisabled(),true);
  await input.press('Enter');assert.equal((await calls('send_text_message')).length,0);
  assert.equal(await input.inputValue(),'保留这条草稿');
  await patch({connection:'connected'});await send.click();
  assert.equal(await input.inputValue(),'保留这条草稿','queue acceptance is not delivery');
  await patch({error:'房间聊天暂不可用'});
  await page.locator('.composer-status').filter({hasText:'房间聊天暂不可用'}).waitFor();
  assert.equal(await input.inputValue(),'保留这条草稿');
  await patch({error:''});await send.click();
  await page.evaluate(()=>window.patch({messages:[...window.state.messages,{id:'confirmed',channelId:'2',senderClientId:window.state.clientId,senderSessionId:window.state.selfSessionId,senderName:'test',senderAvatar:'',text:'保留这条草稿',sentAtUnixMs:Date.now()}]}));
  await page.waitForFunction(()=>document.querySelector('.composer input').value==='');
  await input.fill('汉'.repeat(683));assert.equal(await send.isDisabled(),true);
  await page.locator('.composer-status').filter({hasText:'2048 字节'}).waitFor();
  await input.fill('队列失败草稿');await page.evaluate(()=>window.rejectSend=true);await send.click();
  await page.locator('.composer-status').filter({hasText:'草稿已保留'}).waitFor();
  assert.equal(await input.inputValue(),'队列失败草稿');
  await page.evaluate(()=>window.rejectSend=false);
  await patch({error:'AUDIT_AUDIO_FAILED',audio:{...await page.evaluate(()=>window.state.audio),state:'error("test")'}});
  await page.locator('.operation-error').filter({hasText:'test queue full'}).getByRole('button',{name:'关闭错误提示'}).click();
  await page.locator('.operation-error').filter({hasText:'AUDIT_AUDIO_FAILED'}).waitFor();
  assert.match(await page.locator('.voice-status').innerText(),/语音不可用/);
  assert.match(await page.locator('.connection-chip.unavailable').innerText(),/语音异常/);
  await page.getByRole('button',{name:'重试语音',exact:true}).click();assert.equal((await calls('retry_audio')).length,1);
  await page.setViewportSize({width:820,height:560});
  assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),true);
  await page.screenshot({path:'/tmp/edenvoice-recovery-error.png'});

  await page.goto(url+'/?bootfail');await page.getByText('客户端启动失败',{exact:true}).waitFor();
  await page.screenshot({path:'/tmp/edenvoice-recovery-startup.png'});
  assert.equal(await page.evaluate(()=>(window.handlers['core-state']||[]).length+(window.handlers['core-state-patch']||[]).length),0,'failed bootstrap releases state listeners');
  await page.evaluate(()=>window.failBoot=false);await page.getByRole('button',{name:'重试启动'}).click();
  await page.locator('.workspace').waitFor();
  assert.equal(await page.evaluate(()=>window.handlers['core-state'].length),1,'retry registers one state listener in StrictMode');
  assert.deepEqual(errors,[]);
  console.log('PASS: PTT blur/hidden/chord/repeat, reconnect drafts, delivery confirmation, server and queue rejection, UTF-8 limits, audio failure/retry, startup failure/retry and listener cleanup');
} finally {
  await browser?.close();await new Promise(resolve=>server.close(resolve));
}
