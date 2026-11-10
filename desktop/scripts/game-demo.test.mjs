import assert from 'node:assert/strict';
import {createRequire} from 'node:module';
const require=createRequire(import.meta.url),{chromium}=require(process.env.PLAYWRIGHT_MODULE||'playwright');
const browser=await chromium.launch({executablePath:process.env.CHROME_EXECUTABLE});
try{
 const context=await browser.newContext({viewport:{width:1200,height:850}}),page=await context.newPage(),errors=[];page.on('pageerror',e=>errors.push(String(e)));page.setDefaultTimeout(10000);
 await page.goto('http://127.0.0.1:9182');await page.locator('#players select').first().waitFor();
 const initialVersion=await page.evaluate(()=>window.demoState.catalog.version);
 const selects=page.locator('#players select');assert.equal(await selects.count(),5);
 for(let i=0;i<5;i++){await selects.nth(i).selectOption('');await page.waitForTimeout(100);}
 await page.getByRole('button',{name:'快进 30 秒',exact:true}).click();
 for(let i=0;i<3;i++){await selects.nth(i).selectOption('cs2');await page.waitForTimeout(150);}
 assert.equal(await page.locator('.friend-stage.game').count(),0);
 await page.getByRole('button',{name:'快进 15 秒',exact:true}).click();await page.locator('.friend-stage.game').waitFor();
 await page.screenshot({path:'/tmp/edenvoice-auto-game-demo.png',fullPage:true});
 await page.getByRole('button',{name:'开启共同观影',exact:true}).click();await page.getByRole('button',{name:'确认开启',exact:true}).click();await page.locator('.friend-stage.cinema').waitFor();
 await selects.nth(2).selectOption('');await page.waitForTimeout(150);await page.getByRole('button',{name:'快进 30 秒',exact:true}).click();assert.equal(await page.locator('.friend-stage.cinema').count(),1);
 await page.getByRole('button',{name:'观影操作',exact:true}).click();await page.getByRole('button',{name:'结束全员观影',exact:true}).click();await page.getByRole('button',{name:'确认结束观影',exact:true}).click();await page.locator('.friend-stage.lounge').waitFor();
 const admin=await context.newPage();admin.on('pageerror',e=>errors.push(String(e)));await admin.goto('http://127.0.0.1:9182/console');await admin.locator('[data-page="games"]').click();await admin.locator('[data-action="game-edit"]').first().click();await admin.locator('input[name="title"]').fill('CS2 · 新名称');await admin.getByRole('button',{name:'保存草稿',exact:true}).click();await admin.locator('#modal').waitFor({state:'hidden'});
 await admin.getByRole('button',{name:'预览并发布',exact:true}).click();await admin.getByRole('button',{name:'确认发布',exact:true}).click();await admin.locator('#modal').waitFor({state:'hidden'});await page.getByText('目录 v'+(initialVersion+1),{exact:true}).waitFor();await admin.screenshot({path:'/tmp/edenvoice-game-admin-demo.png',fullPage:true});
 await admin.getByRole('button',{name:'回退上一版',exact:true}).click();await admin.getByRole('button',{name:'确认回退',exact:true}).click();await admin.locator('#modal').waitFor({state:'hidden'});await page.getByText('目录 v'+(initialVersion+2),{exact:true}).waitFor();
 assert.deepEqual(errors,[]);console.log('PASS: real scene engine, 3/5 majority, 15/30 seconds, cinema priority, draft/publish/rollback and live refresh');
}finally{await browser.close()}
