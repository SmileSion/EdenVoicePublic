import assert from 'node:assert/strict';
import {createRequire} from 'node:module';
import {readFile} from 'node:fs/promises';
import {createServer} from 'node:http';
import {fileURLToPath} from 'node:url';
import {build} from 'esbuild';
const require=createRequire(import.meta.url),{chromium}=require(process.env.PLAYWRIGHT_MODULE||'playwright');
const root=fileURLToPath(new URL('..',import.meta.url));
const bundle=await build({stdin:{resolveDir:root,loader:'tsx',contents:'import React from \'react\';import{createRoot}from\'react-dom/client\';import{VoiceIncident}from\'./src/VoiceIncident\';window.__TAURI_INTERNALS__={};window.calls=[];window.state={available:true,collecting:false,remainingSeconds:0,report:null};window.rpc=async(cmd,args)=>{window.calls.push([cmd,args]);if(cmd===\'export_diagnostics\'){window.exported=JSON.parse(args.report);return \'/test/report.json\';}if(cmd===\'voice_incident\'){if(args.action===\'mark\')window.state={...window.state,collecting:true,remainingSeconds:10,report:{schema:\'edenvoice.voice-diagnostics.v1\',completion:\'collecting\',samples:[]}};if(args.action===\'clear\')window.state={...window.state,report:null};return window.state;}};createRoot(document.getElementById(\'root\')).render(<div className="app-shell theme-dark"><main style={{width:440,margin:30}}><VoiceIncident/></main></div>);\n'},bundle:true,write:false,format:'iife',jsx:'automatic',plugins:[{name:'native',setup(b){b.onResolve({filter:/^@tauri-apps\/api\/core$/},a=>({path:a.path,namespace:'mock'}));b.onLoad({filter:/.*/,namespace:'mock'},()=>({contents:`export const invoke=(...a)=>window.rpc(...a);`,loader:'js'}));}}]});
const css=await readFile(root+'/src/styles.css','utf8');const server=createServer((req,res)=>{res.setHeader('Content-Type',req.url==='/app.js'?'text/javascript':req.url==='/style.css'?'text/css':'text/html');res.end(req.url==='/app.js'?bundle.outputFiles[0].text:req.url==='/style.css'?css:'<html><link rel="stylesheet" href="/style.css"><div id="root"></div><script src="/app.js"></script></html>');});await new Promise(r=>server.listen(0,'127.0.0.1',r));
const browser=await chromium.launch({executablePath:process.env.CHROME_EXECUTABLE});
try {
 const page=await browser.newPage({viewport:{width:580,height:620}}),errors=[];page.on('pageerror',e=>errors.push(String(e)));
 await page.goto('http://127.0.0.1:'+server.address().port);
 await page.getByRole('button',{name:'记录问题',exact:true}).click();
 assert.equal(await page.getByRole('button',{name:'补充中 · 10 秒'}).isDisabled(),true);
 await page.evaluate(()=>window.state={...window.state,collecting:false,remainingSeconds:0,report:{schema:'edenvoice.voice-diagnostics.v1',completion:'complete',samples:[{atMs:1}]}});
 await page.getByRole('button',{name:'导出报告'}).click();await page.getByText('已导出：/test/report.json').waitFor();
 assert.equal(await page.evaluate(()=>window.exported.samples.length),1);
 await page.screenshot({path:'/tmp/eden-voice-incident.png'});
 await page.getByRole('button',{name:'清除',exact:true}).click();assert.equal(await page.getByRole('button',{name:'导出报告'}).count(),0);
 await page.evaluate(()=>window.state={...window.state,available:false});await page.waitForTimeout(1200);assert.equal(await page.getByRole('button',{name:'记录问题',exact:true}).isDisabled(),true);
 assert.deepEqual(errors,[]);console.log('PASS: mark, collecting state, ready, export exact report, clear and disconnected state');
}finally{await browser.close();server.close();}
