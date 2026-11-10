import assert from 'node:assert/strict';
import {createRequire} from 'node:module';
const require=createRequire(import.meta.url),{chromium}=require(process.env.PLAYWRIGHT_MODULE||'playwright');
if(!process.env.EDENVOICE_DEMO_PASSWORD)throw Error('Demo password required');
const browser=await chromium.launch({executablePath:process.env.CHROME_EXECUTABLE});
try{
 const page=await browser.newPage({viewport:{width:1300,height:800}}),errors=[];page.on('pageerror',e=>errors.push(String(e)));
 await page.goto(process.env.EDENVOICE_DEMO_URL||'http://127.0.0.1:19321');await page.locator('[name=username]').fill('admin');await page.locator('[name=password]').fill(process.env.EDENVOICE_DEMO_PASSWORD);await page.locator('#login-form button[type=submit]').click();
 await page.locator('[data-page=connections]').click();await page.getByRole('cell',{name:'0.8.1 windows',exact:true}).waitFor();await page.getByRole('cell',{name:'0.8.0 macos',exact:true}).waitFor();assert.ok(await page.getByRole('cell',{name:'未知 未上报',exact:true}).count()>0);
 await page.screenshot({path:'/tmp/eden-client-build-admin.png',fullPage:true});
 await page.locator('[data-search]').fill('0.8.1');await page.getByRole('button',{name:'详情',exact:true}).click();await page.getByText('客户端版本',{exact:true}).waitFor();await page.locator('#modal').getByText('0.8.1',{exact:true}).waitFor();
 assert.deepEqual(errors,[]);console.log('PASS: version/platform visible, old clients unknown, version search and session detail');
}finally{await browser.close();}
