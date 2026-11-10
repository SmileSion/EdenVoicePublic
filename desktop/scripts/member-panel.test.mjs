import assert from 'node:assert/strict';
import {createRequire} from 'node:module';
import {readFile} from 'node:fs/promises';
import {createServer} from 'node:http';
import {fileURLToPath} from 'node:url';
import {buildBrowserFixture as build, fixtureStyles} from './browser-fixture.mjs';
const require=createRequire(import.meta.url),{chromium}=require(process.env.PLAYWRIGHT_MODULE||'playwright');
const root=fileURLToPath(new URL('..',import.meta.url));
const bundle=await build({stdin:{resolveDir:root,loader:'tsx',contents:`
import React from 'react';import{createRoot}from'react-dom/client';import{App}from'./src/App';import{mockSnapshot}from'./src/mock';
window.__TAURI_INTERNALS__={};window.nativeExpand=true;window.calls=[];
const snapshot=structuredClone(mockSnapshot);snapshot.users[0].gameActivity={kind:'game',title:'无畏契约',source:'windows',updatedAt:Date.now()-1000,expiresAt:Date.now()+600000};
window.rpc=async(name,args)=>{if(name==='get_snapshot')return snapshot;if(name==='get_close_preferences')return {behavior:'minimize',pending:false,trayAvailable:true};if(name==='resize_member_panel'){window.calls.push(args.open);if(!window.nativeExpand)return false;await window.resizeTest(args.open);return true;}if(name==='get_avatar_data')return null;return null;};
createRoot(document.getElementById('root')).render(<App/>);
`},loader:{'.svg':'text'},bundle:true,write:false,format:'iife',jsx:'automatic',plugins:[{name:'native',setup(b){b.onResolve({filter:/^@tauri-apps\/api\/(core|event|window)$/},a=>({path:a.path,namespace:'mock'}));b.onLoad({filter:/.*/,namespace:'mock'},()=>({contents:'export const invoke=(...a)=>window.rpc(...a);export const listen=async()=>()=>{};export const getCurrentWindow=()=>({});'}));}}]});
const css=(await Promise.all(['styles.css','watch-party.css','room-experience.css','close-behavior.css'].map(f=>readFile(root+'/src/'+f,'utf8')))).join('\n');
const importedCss = fixtureStyles(bundle);
const server=createServer((req,res)=>{res.setHeader('Content-Type',req.url==='/app.js'?'text/javascript':req.url==='/style.css'?'text/css':'text/html');res.end(req.url==='/app.js'?bundle.outputFiles[0].text:req.url==='/style.css'?css+'\n'+importedCss:'<html><link rel="stylesheet" href="/style.css"><div id="root"></div><script src="/app.js"></script></html>');});
await new Promise(r=>server.listen(0,'127.0.0.1',r));
const browser=await chromium.launch({executablePath:process.env.CHROME_EXECUTABLE});
try{const page=await browser.newPage({viewport:{width:820,height:700}});const errors=[];page.on('pageerror',e=>errors.push(String(e)));await page.exposeFunction('resizeTest',async open=>{const v=page.viewportSize();await page.setViewportSize({width:v.width+(open?208:-208),height:v.height});});await page.goto('http://127.0.0.1:'+server.address().port);await page.locator('.workspace').waitFor();const before=await page.locator('.chat-panel').boundingBox();assert.equal(await page.locator('.member-panel').count(),0);await page.getByRole('button',{name:'显示服务器成员'}).click();await page.locator('.members-outward').waitFor();const after=await page.locator('.chat-panel').boundingBox();assert.ok(Math.abs(before.width-after.width)<1,'room width preserved across responsive breakpoint');assert.match(await page.locator('.member-panel').innerText(),/无畏契约/);await page.getByRole('button',{name:'隐藏服务器成员'}).click();await page.locator('.members-hidden').waitFor();assert.equal(page.viewportSize().width,820);await page.evaluate(()=>window.nativeExpand=false);await page.getByRole('button',{name:'显示服务器成员'}).click();await page.locator('.members-overlay').waitFor();assert.ok(Math.abs((await page.locator('.chat-panel').boundingBox()).width-before.width)<1);assert.deepEqual(errors,[]);console.log('PASS: game status, outward expansion, restore and no-space overlay');}finally{await browser.close();server.close();}
