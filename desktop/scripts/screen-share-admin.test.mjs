import assert from 'node:assert/strict';
import {createRequire} from 'node:module';
const require=createRequire(import.meta.url);
const {chromium}=require(process.env.PLAYWRIGHT_MODULE||'playwright');
const base=process.env.EDENVOICE_DEMO_URL||'http://127.0.0.1:9187';
if(!process.env.EDENVOICE_DEMO_PASSWORD)throw new Error('Provide local demo credentials');
const browser=await chromium.launch({headless:true,executablePath:process.env.CHROME_EXECUTABLE});
try{
 const page=await browser.newPage({viewport:{width:1280,height:800}});
 const errors=[];page.on('pageerror',e=>errors.push(e.message));
 await page.goto(base);await page.locator('[name=username]').fill('admin');await page.locator('[name=password]').fill(process.env.EDENVOICE_DEMO_PASSWORD);
 await page.locator('#login-form button[type=submit]').click();
 await page.locator('[data-page="screen-sharing"]').click();
 await page.getByRole('button',{name:'调整共享限制'}).click();
 await page.locator('[name=enabled]').selectOption('true');
 await page.locator('[name=max_fps]').fill('20');
 await page.locator('[name=max_bitrate_kbps]').fill('1800');
 await page.locator('#modal-submit').click();
 await page.locator('#modal').waitFor({state:'hidden'});
 await page.getByText('1800 kbps',{exact:true}).waitFor();
 await page.screenshot({path:'/tmp/eden-screen-admin-page.png',fullPage:true});
 await page.reload();await page.locator('[data-page="screen-sharing"]').click();
 await page.getByText('1800 kbps',{exact:true}).waitFor();
 await page.getByRole('button',{name:'调整共享限制'}).click();
 await page.setViewportSize({width:820,height:560});
 await page.screenshot({path:'/tmp/eden-screen-admin-dialog.png',fullPage:true});
 // Settings may scroll, but the save action must stay visible without scrolling.
 const box=await page.locator('#modal-submit').boundingBox();assert.ok(box&&box.y>=0&&box.y+box.height<=560);
 await page.locator('[name=max_fps]').fill('61');
 assert.equal(await page.locator('[name=max_fps]').evaluate(el=>el.checkValidity()),false);
 assert.deepEqual(errors,[]);
 console.log('PASS: save, reload, compact modal access, validation, no page errors');
}finally{await browser.close();}
