import assert from 'node:assert/strict';
import {createRequire} from 'node:module';
const {chromium}=createRequire(import.meta.url)(process.env.PLAYWRIGHT_MODULE||'playwright');
const browser=await chromium.launch({executablePath:process.env.CHROME_EXECUTABLE});
try {
 const page=await browser.newPage({viewport:{width:1200,height:900}}),errors=[];page.on('pageerror',e=>errors.push(String(e)));
 await page.goto('http://127.0.0.1:9185');await page.locator('.memory-card').first().waitFor();
 await page.locator('.memory-avatars img').first().waitFor();
 assert.equal(await page.locator('.memory-card').count(),5);
 await page.screenshot({path:'/tmp/eden-archive-games-dark.png'});
 await page.getByRole('button',{name:'一起玩',exact:true}).click();
 assert.equal(await page.locator('.memory-card').count(),4);
 await page.getByLabel('筛选游戏').selectOption('valorant');assert.equal(await page.locator('.memory-card').count(),2);
 await page.getByLabel('搜索回忆').fill('不存在的名字');assert.equal(await page.locator('.memory-card').count(),0);
 await page.getByRole('button',{name:'看看全部回忆'}).click();
 await page.getByLabel('搜索回忆').fill('小鹿');assert.equal(await page.locator('.memory-card').count(),5);
 await page.getByLabel('搜索回忆').fill('');
 await page.getByRole('button',{name:'制作回顾卡片：一起玩了 无畏契约',exact:true}).first().click();
 await page.getByRole('dialog').waitFor();assert.equal(await page.getByRole('combobox').inputValue(),'valorant-14');
 await page.getByRole('button',{name:'关闭回顾卡片'}).click();
 await page.getByRole('button',{name:'切换白天'}).click();await page.screenshot({path:'/tmp/eden-archive-games-light.png'});
 await page.getByRole('button',{name:'体验多人记录'}).click();await page.getByRole('button',{name:'+19',exact:true}).first().click();assert.equal(await page.locator('.memory-card').first().locator('.memory-avatars>span').count(),24);
 await page.setViewportSize({width:820,height:700});
 assert.ok(await page.locator('.archive-page').evaluate(n=>n.scrollWidth<=n.clientWidth));
 assert.ok(await page.locator('.memory-scroll').evaluate(n=>n.clientHeight>100));
 await page.screenshot({path:'/tmp/eden-archive-games-compact.png'});
 assert.deepEqual(errors,[]);console.log('PASS: game/name filters, empty state, per-event recap, avatars, 24 members, light and compact layouts');
} finally {await browser.close();}
