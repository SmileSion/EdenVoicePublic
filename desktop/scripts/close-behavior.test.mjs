// Tests the real React view with an explicitly mocked native bridge.
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { build } from "esbuild";
const require = createRequire(import.meta.url);
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || "playwright");
const root = fileURLToPath(new URL("..", import.meta.url));
const bundle = await build({
  stdin: { contents: `
    import React from 'react'; import {createRoot} from 'react-dom/client'; import {flushSync} from 'react-dom';
    import {CloseBehaviorDialog, CloseBehaviorSettings} from './src/CloseBehavior';
    const root=createRoot(document.getElementById('root'));
    window.__TAURI_INTERNALS__={}; window.calls=[]; window.subscribers=[];
    window.preferences={behavior:'ask',pending:false,trayAvailable:true};
    window.fail=false;
    window.rpc=async(name,args)=>{
      window.calls.push([name,args]);
      if(name==='get_close_preferences')return structuredClone(window.preferences);
      if(name==='set_close_behavior'){
        if(window.fail)throw Error('无法保存设置');
        window.preferences.behavior=args.behavior; return;
      }
      if(name==='resolve_window_close'){
        if(window.fail)throw Error('无法执行关闭操作');
        if(args.remember&&args.action!=='cancel')window.preferences.behavior=args.action;
        window.preferences.pending=false;
      }
    };
    window.changePreference=(behavior)=>{window.preferences.behavior=behavior;window.subscribers.forEach(f=>f({payload:behavior}));};
    window.requestClose=()=>{window.preferences.pending=true;window.subscribers.forEach(f=>f({payload:structuredClone(window.preferences)}));};
    window.draw=()=>flushSync(()=>root.render(<React.StrictMode><CloseBehaviorDialog theme="light"/></React.StrictMode>));
    window.drawSettings=()=>flushSync(()=>root.render(<CloseBehaviorSettings/>));
    window.draw();
  `,resolveDir:root,loader:"tsx"}, bundle:true,write:false,format:"iife",jsx:"automatic",
  define:{"process.env.NODE_ENV":'"development"'},plugins:[{
    name:"native-test-bridge", setup(build) {
      build.onResolve({filter:/^@tauri-apps\/api\/(core|event|window)$/},args=>({path:args.path,namespace:"mock"}));
      build.onLoad({filter:/.*/,namespace:"mock"},()=>({contents:`
        export const invoke=(...args)=>window.rpc(...args);
        export const listen=async (_,fn)=>{window.subscribers.push(fn);return ()=>{window.subscribers=window.subscribers.filter(f=>f!==fn);};};
        export const getCurrentWindow=()=>({});
      `,loader:"js"}));
    }
  }]
});
const css=(await Promise.all(["styles.css","close-behavior.css"].map(f=>readFile(join(root,"src",f),"utf8")))).join("\n");
const server=createServer((req,res)=>{
  if(req.url==="/app.js"){res.setHeader("Content-Type","text/javascript");res.end(bundle.outputFiles[0].text);}
  else if(req.url==="/style.css"){res.setHeader("Content-Type","text/css");res.end(css);}
  else res.end('<html><head><link rel="stylesheet" href="/style.css"></head><body><div id="root"></div><script src="/app.js"></script></body></html>');
});
await new Promise(resolve=>server.listen(0,"127.0.0.1",resolve));
let browser;
try {
  browser=await chromium.launch(process.env.CHROME_EXECUTABLE?{executablePath:process.env.CHROME_EXECUTABLE}:{});
  const page=await browser.newPage({viewport:{width:1000,height:850}});
  const errors=[]; page.on("pageerror",e=>errors.push(String(e))); page.setDefaultTimeout(5000);
  await page.goto(`http://127.0.0.1:${server.address().port}`);

  await page.waitForFunction(()=>window.subscribers.length===1);
  await page.evaluate(()=>window.requestClose());
  const dialog=page.getByRole('dialog',{name:'关闭 EdenVoice'});
  await dialog.waitFor();
  await page.getByRole('checkbox').check();
  await page.getByRole('radio',{name:/退出程序/}).check();
  await page.evaluate(()=>window.requestClose());
  assert.equal(await page.getByRole('checkbox').isChecked(),true);
  assert.equal(await page.getByRole('radio',{name:/退出程序/}).isChecked(),true);
  await page.keyboard.press('Escape');
  await dialog.waitFor({state:'hidden'});
  assert.equal(await page.evaluate(()=>window.preferences.behavior),'ask');
  assert.deepEqual(await page.evaluate(()=>window.calls.find(([n])=>n==='resolve_window_close')[1]),{action:'cancel',remember:false});

  await page.evaluate(()=>window.requestClose());
  assert.equal(await page.getByRole('checkbox').isChecked(),false);
  await page.getByRole('button',{name:'收起到托盘',exact:true}).click();
  await dialog.waitFor({state:'hidden'});
  assert.equal(await page.evaluate(()=>window.preferences.behavior),'ask');

  await page.evaluate(()=>window.requestClose());
  await page.getByRole('checkbox').check();
  await page.getByRole('button',{name:'收起到托盘',exact:true}).click();
  await dialog.waitFor({state:'hidden'});
  assert.equal(await page.evaluate(()=>window.preferences.behavior),'tray');

  await page.evaluate(()=>window.drawSettings());
  await page.waitForFunction(()=>document.getElementById('close-behavior')?.value==='tray');
  await page.evaluate(()=>window.changePreference('quit'));
  await page.waitForFunction(()=>document.getElementById('close-behavior')?.value==='quit');
  await page.getByLabel('点击关闭按钮时').selectOption('quit');
  await page.getByRole('status').filter({hasText:'已保存'}).waitFor();
  assert.equal(await page.evaluate(()=>window.preferences.behavior),'quit');
  await page.getByLabel('点击关闭按钮时').selectOption('ask');
  await page.waitForFunction(()=>window.preferences.behavior==='ask');
  await page.evaluate(()=>window.fail=true);
  await page.getByLabel('点击关闭按钮时').selectOption('quit');
  await page.getByRole('alert').filter({hasText:'无法保存设置'}).waitFor();
  assert.equal(await page.evaluate(()=>window.preferences.behavior),'ask');

  // Pending native request is recovered after remount/startup without a new event.
  await page.evaluate(()=>{window.fail=false;window.preferences.pending=true;window.draw();});
  await dialog.waitFor();
  await page.getByRole('radio',{name:/退出程序/}).check();
  await page.getByRole('checkbox').check();
  await page.screenshot({path:join(tmpdir(),'edenvoice-close-choice.png')});
  await page.evaluate(()=>window.fail=true);
  await page.getByRole('button',{name:'退出程序',exact:true}).click();
  await page.getByRole('alert').filter({hasText:'无法执行关闭操作'}).waitFor();
  assert.equal(await dialog.isVisible(),true);
  await page.evaluate(()=>window.fail=false);
  await page.getByRole('button',{name:'退出程序',exact:true}).click();
  await dialog.waitFor({state:'hidden'});
  assert.equal(await page.evaluate(()=>window.preferences.behavior),'quit');

  await page.evaluate(()=>{window.preferences.trayAvailable=false;window.requestClose();});
  assert.equal(await page.getByRole('radio',{name:/最小化到托盘/}).isDisabled(),true);
  assert.equal(await page.getByRole('radio',{name:/退出程序/}).isChecked(),true);
  await page.setViewportSize({width:306,height:480});
  assert.ok(await page.locator('.close-choice').evaluate(el=>el.scrollWidth<=el.clientWidth));
  await page.screenshot({path:join(tmpdir(),'edenvoice-close-choice-compact.png')});
  await page.getByRole('button',{name:'取消',exact:true}).click();
  await dialog.waitFor({state:'hidden'});
  assert.deepEqual(errors,[]);
  console.log('close behavior UI: cancel, repeated request, remember, settings, failed save, startup recovery and compact layout passed');

} finally {await browser?.close();await new Promise(resolve=>server.close(resolve));}
