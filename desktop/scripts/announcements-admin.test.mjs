import assert from 'node:assert/strict';
import {createRequire} from 'node:module';
const require=createRequire(import.meta.url),{chromium}=require(process.env.PLAYWRIGHT_MODULE||'playwright');
if(!process.env.EDENVOICE_DEMO_PASSWORD)throw Error('Local demo password required');
const browser=await chromium.launch({executablePath:process.env.CHROME_EXECUTABLE});
try{
 const page=await browser.newPage({viewport:{width:1100,height:760}}),errors=[];page.on('pageerror',e=>errors.push(String(e)));
 await page.goto(process.env.EDENVOICE_DEMO_URL||'http://127.0.0.1:19291');
 await page.locator('[name=username]').fill('admin');await page.locator('[name=password]').fill(process.env.EDENVOICE_DEMO_PASSWORD);await page.locator('#login-form button[type=submit]').click();
 await page.locator('[data-page=announcements]').click();await page.getByRole('button',{name:'编辑草稿'}).click();
 await page.locator('[name=title]').fill('今晚，一起相聚');await page.locator('[name=body]').fill('周末放映室 20:30 见。\n语音连接保持，不用退出房间。');await page.locator('#modal-submit').click();await page.locator('#modal').waitFor({state:'hidden'});
 await page.getByText('还没有正在下发的公告',{exact:true}).waitFor();
 await page.getByRole('button',{name:'预览并发布'}).click();await page.locator('#modal-submit').click();await page.locator('#modal').waitFor({state:'hidden'});await page.getByText('下发中',{exact:true}).waitFor();
 await page.reload();await page.locator('[data-page=announcements]').click();await page.getByText('下发中',{exact:true}).waitFor();
 await page.screenshot({path:'/tmp/eden-announcements-admin.png',fullPage:true});
 await page.getByRole('button',{name:'撤回公告',exact:true}).click();await page.locator('#modal-submit').click();await page.locator('#modal').waitFor({state:'hidden'});await page.getByText('还没有正在下发的公告',{exact:true}).waitFor();
 assert.deepEqual(errors,[]);console.log('PASS: authenticated admin edits draft, previews, publishes, reloads and withdraws announcement.');
}finally{await browser.close();}
