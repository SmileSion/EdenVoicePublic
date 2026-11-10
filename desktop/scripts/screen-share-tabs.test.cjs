// Run against scripts/screen-share-demo.mjs (actual Chat/ScreenShare components).
const assert=require('node:assert/strict');
const {chromium}=require(process.env.PLAYWRIGHT_MODULE||'playwright');
(async()=>{
 const browser=await chromium.launch({headless:true,executablePath:process.env.CHROME_EXECUTABLE});
 try{
 const page=await browser.newPage({viewport:{width:1200,height:820}});const errors=[];page.on('pageerror',e=>errors.push(e.message));
 await page.addInitScript(()=>Object.defineProperty(navigator,'platform',{value:'Win32'}));
 await page.goto('http://127.0.0.1:9198');
 await page.getByRole('button',{name:'朋友发起共享',exact:true}).click();
 await page.getByRole('button',{name:'观看',exact:true}).click();
 await page.locator('.screen-viewer').waitFor({state:'visible'});
 assert.equal(await page.getByRole('button',{name:'屏幕共享',exact:true}).getAttribute('aria-pressed'),'true');
 await page.waitForFunction(()=>document.querySelector('.screen-video video').videoWidth>0);
 await page.evaluate(()=>{window.savedController=window.mockController;window.savedStream=document.querySelector('.screen-video video').srcObject;});
 await page.screenshot({path:'/tmp/eden-screen-share-preview/tab-viewing.png'});
 for(const label of ['房间','聊天']){
 await page.getByRole('button',{name:label,exact:true}).click();
 await page.locator('.screen-viewer').waitFor({state:'hidden'});
 assert.equal(await page.evaluate(()=>window.savedController===window.mockController&&document.querySelector('.screen-video video').srcObject===window.savedStream&&window.savedStream.getTracks().every(t=>t.readyState==='live')),true);
 await page.screenshot({path:`/tmp/eden-screen-share-preview/tab-${label==='房间'?'room':'chat'}.png`});
 }
 await page.getByRole('button',{name:'屏幕共享',exact:true}).click();
 await page.getByRole('button',{name:'全屏',exact:true}).click();
 await page.waitForFunction(()=>!!document.fullscreenElement);
 await page.evaluate(()=>document.exitFullscreen());
 assert.equal(await page.getByRole('button',{name:'屏幕共享',exact:true}).getAttribute('aria-pressed'),'true');
 for(const [width,height] of [[1200,820],[860,660],[860,580]]){
 await page.setViewportSize({width,height});
 const bounds=await page.evaluate(()=>{const v=document.querySelector('.screen-viewer').getBoundingClientRect(),c=document.querySelector('.composer').getBoundingClientRect(),bar=document.querySelector('.screen-strip').getBoundingClientRect();return {top:v.top,bottom:v.bottom,composer:c.top,bar:bar.bottom,height:v.height,scroll:document.querySelector('.chat-panel').scrollHeight,client:document.querySelector('.chat-panel').clientHeight};});
 assert.ok(bounds.top>=bounds.bar-1&&bounds.bottom<=bounds.composer+1&&bounds.height>80,JSON.stringify(bounds));
 assert.ok(bounds.scroll<=bounds.client+1,JSON.stringify(bounds));
 }
 await page.screenshot({path:'/tmp/eden-screen-share-preview/tab-compact.png'});
 await page.getByRole('button',{name:'切换明暗主题'}).click();
 await page.waitForTimeout(250);
 await page.screenshot({path:'/tmp/eden-screen-share-preview/tab-light.png'});
 await page.getByRole('button',{name:'停止观看',exact:true}).click();
 assert.equal(await page.evaluate(()=>window.savedStream.getTracks().every(t=>t.readyState==='ended')),true);
 await page.locator('.screen-empty').waitFor({state:'visible'});
 assert.deepEqual(errors,[]);console.log('PASS: embedded tabs preserve session/tracks; fullscreen returns to tab; responsive layout and stop cleanup');
 }finally{await browser.close();}
})();
