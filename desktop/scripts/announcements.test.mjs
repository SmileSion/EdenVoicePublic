import assert from 'node:assert/strict';
import {createRequire} from 'node:module';
import {readFile} from 'node:fs/promises';
import {createServer} from 'node:http';
import {fileURLToPath} from 'node:url';
import {build} from 'esbuild';
const require=createRequire(import.meta.url),{chromium}=require(process.env.PLAYWRIGHT_MODULE||'playwright');
const root=fileURLToPath(new URL('..',import.meta.url));
const bundle=await build({stdin:{resolveDir:root,loader:'tsx',contents:`import React,{useState}from'react';import{createRoot}from'react-dom/client';import{AnnouncementNotice}from'./src/AnnouncementNotice';function Test(){const[s,set]=useState({connection:'connected',clientId:'alice',announcementScope:'server-a',serverName:'朋友的服务器',announcement:null});window.update=p=>set(s=>({...s,...p}));return <div className="app-shell theme-dark"><AnnouncementNotice snapshot={s}/></div>;}createRoot(document.getElementById('root')).render(<Test/>);`},bundle:true,write:false,format:'iife',jsx:'automatic'});
const css=await readFile(root+'/src/styles.css','utf8');
const server=createServer((req,res)=>{res.setHeader('Content-Type',req.url==='/app.js'?'text/javascript':req.url==='/style.css'?'text/css':'text/html');res.end(req.url==='/app.js'?bundle.outputFiles[0].text:req.url==='/style.css'?css:'<html><link rel="stylesheet" href="/style.css"><div id="root"></div><script src="/app.js"></script></html>');});
await new Promise(r=>server.listen(0,'127.0.0.1',r));
const browser=await chromium.launch({executablePath:process.env.CHROME_EXECUTABLE});
try {
 const page=await browser.newPage({viewport:{width:820,height:650}}),errors=[];
 page.on('pageerror',e=>errors.push(String(e)));
 const open=async()=>{await page.goto('http://127.0.0.1:'+server.address().port);await page.waitForFunction(()=>!!window.update);};
 const send=async(revision,body='周末一起玩。')=>{await page.evaluate(a=>window.update({announcement:a,announcementRevision:a.revision}),{revision:String(revision),title:'公告 '+revision,body,published_at:Date.now()});await page.waitForTimeout(50);};
 const visible=()=>page.locator('dialog[open]').count();
 await open(); await send(3);assert.equal(await visible(),1);
 await page.screenshot({path:'/tmp/eden-announcement-dark.png'});
 await page.evaluate(()=>document.querySelector('.app-shell').className='app-shell theme-light');
 await page.screenshot({path:'/tmp/eden-announcement-light.png'});
 await page.getByRole('button',{name:'知道了',exact:true}).click();assert.equal(await visible(),0);
 await send(3);assert.equal(await visible(),0);
 await open();await send(3);assert.equal(await visible(),0);
 await send(5);await page.getByRole('heading',{name:'公告 5'}).waitFor();
 await send(7);await page.getByRole('heading',{name:'公告 7'}).waitFor();assert.equal(await visible(),1);
 await send(5);await page.getByRole('heading',{name:'公告 7'}).waitFor();
 await page.keyboard.press('Escape');assert.equal(await visible(),0);
 await page.evaluate(()=>window.update({connection:'disconnected'}));await page.evaluate(()=>window.update({connection:'connected'}));await send(7);assert.equal(await visible(),0);
 await page.evaluate(()=>window.update({announcementScope:'server-b'}));await send(7);assert.equal(await visible(),1);
 await page.evaluate(()=>window.update({announcement:null,announcementRevision:"8"}));await page.waitForTimeout(50);assert.equal(await visible(),0);
 await send(7);assert.equal(await visible(),0); // stale publish after withdrawal
 await send(9);assert.equal(await visible(),1);
 await page.evaluate(()=>window.update({announcement:null,announcementRevision:'8'}));await page.waitForTimeout(50);assert.equal(await visible(),1); // stale withdrawal after publish
 await send(9,'<img src=x onerror="window.pwned=true">\n'+ '这是可复制的完整公告。'.repeat(250));assert.equal(await page.locator('dialog img').count(),0);
 await page.setViewportSize({width:600,height:400});await page.getByRole('button',{name:'知道了',exact:true}).scrollIntoViewIfNeeded();await page.getByRole('button',{name:'知道了',exact:true}).click();assert.equal(await visible(),0);
 await page.evaluate(()=>window.update({clientId:'bob'}));await send(9);assert.equal(await visible(),1);
 await page.getByRole('button',{name:'知道了',exact:true}).click();
 await page.evaluate(()=>{window.originalSet=Storage.prototype.setItem;Storage.prototype.setItem=()=>{throw new Error('Storage denied');};});
 await send(11);await page.getByRole('button',{name:'知道了',exact:true}).click();
 await page.getByRole('button',{name:'仍然关闭',exact:true}).waitFor();
 await send(12);await page.getByRole('button',{name:'知道了',exact:true}).waitFor(); // reset failure for new publication
 await page.getByRole('button',{name:'知道了',exact:true}).click();await page.getByRole('button',{name:'仍然关闭',exact:true}).click();assert.equal(await visible(),0);
 await page.evaluate(()=>Storage.prototype.setItem=window.originalSet);
 assert.deepEqual(errors,[]);
 console.log('PASS: latest-only replacement, stale rejection, persistent reads, reconnect, server isolation, withdrawal, Escape acknowledgement, safe plain text, long announcement at small viewport.');
}finally{await browser.close();server.close();}
