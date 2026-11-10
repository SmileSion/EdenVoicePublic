// Browser integration test against the actual Chat component, no backend writes.
// Set PLAYWRIGHT_MODULE to a Playwright installation if it is not local.
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { buildBrowserFixture as build, fixtureStyles } from "./browser-fixture.mjs";

const require = createRequire(import.meta.url);
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || "playwright");
const root = fileURLToPath(new URL("..", import.meta.url));
const bundle = await build({
  stdin: { contents: `
    import React from 'react';
    import {createRoot} from 'react-dom/client';
    import {flushSync} from 'react-dom';
    import {App,Chat} from './src/App';
    import {mockSnapshot} from './src/mock';
    window.commands=[]; console.info=(label,type)=>window.commands.push({type});
    window.drawApp=()=>flushSync(()=>root.render(<App/>));
    const root = createRoot(document.getElementById('root'));
    window.draw = (ids, server = 'test-server', room = '42') => flushSync(() => root.render(
      <React.StrictMode><Chat snapshot={{...mockSnapshot, serverAddress: server, messages: ids.map(id => ({
        id: String(id), channelId: room, senderClientId: 'other', senderSessionId: 'other',
        senderName: 'Test', senderAvatar: '', text: 'message ' + id + ' '.repeat(2) + 'line '.repeat(id % 3 * 25),
        sentAtUnixMs: 1700000000000 + id * 1000
      }))}} channel={{id: room, name: 'Test room'}} membersVisible={false} onToggleMembers={() => {}} onMore={() => {}} /></React.StrictMode>
    ));
  `, resolveDir: root, loader: "tsx" },
  bundle: true, write: false, format: "iife", jsx: "automatic", loader: { ".svg": "text" },
  define: { "process.env.NODE_ENV": '"development"' },
});
const css = (await Promise.all(["styles.css","room-experience.css"].map(f=>readFile(new URL("../src/"+f, import.meta.url), "utf8")))).join("\n");
const importedCss = fixtureStyles(bundle);
const server = createServer((request, response) => {
  if (request.url === "/app.js") { response.setHeader("Content-Type", "text/javascript"); response.end(bundle.outputFiles[0].text); }
  else if (request.url === "/style.css") { response.setHeader("Content-Type", "text/css"); response.end(css + "\n" + importedCss); }
  else response.end('<html><head><link rel="stylesheet" href="/style.css"><style>#root{height:500px;width:650px}.chat-panel{height:100%}</style></head><body><div id="root"></div><script src="/app.js"></script></body></html>');
});
await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
let browser;
try {
  browser = await chromium.launch(process.env.CHROME_EXECUTABLE ? { executablePath: process.env.CHROME_EXECUTABLE } : {});
  const page = await browser.newPage({ viewport: { width: 900, height: 700 } });
  const errors = [];
  page.on("pageerror", error => { errors.push(String(error)); console.error(error); });
  page.setDefaultTimeout(5000);
  await page.goto(`http://127.0.0.1:${server.address().port}`);
  const range = (start, end) => Array.from({length: end - start + 1}, (_, i) => start + i);
  const draw = async (ids, server, room) => { await page.evaluate(args => window.draw(...args), [ids, server, room]); const chat=page.locator('.chat-view-switch button').filter({hasText:'聊天'}); if(await chat.getAttribute('aria-pressed')!=='true')await chat.click(); await page.waitForTimeout(60); };
  const gap = () => page.locator('.messages').evaluate(n => n.scrollHeight - n.clientHeight - n.scrollTop);
  const anchor = () => page.locator('.messages').evaluate(n => {
    const top = n.getBoundingClientRect().top;
    const row = [...n.querySelectorAll('[data-message-id]')].find(r => r.getBoundingClientRect().bottom > top);
    return {id: row.dataset.messageId, top: row.getBoundingClientRect().top - top};
  });
  const readHistory = async () => {
    await page.locator('.messages').evaluate(n => { n.scrollTop = 900; n.dispatchEvent(new Event('scroll', {bubbles: true})); });
    await page.waitForTimeout(60);
  };
  const sameAnchor = async before => {
    const after = await anchor(); assert.equal(after.id, before.id); assert.ok(Math.abs(after.top - before.top) < 1);
  };
  await draw(range(1, 100));
  assert.ok(await gap() <= 1, 'initial history opens at latest');
  await draw(range(2, 101));
  assert.ok(await gap() <= 1, 'at bottom follows rolling history');
  await page.locator('.messages').hover();
  await page.mouse.wheel(0, -600);
  await page.waitForTimeout(150);
  assert.ok(await gap() > 32, 'real wheel exits bottom-follow mode');
  const wheelAnchor = await anchor();
  await draw(range(2, 101));
  await sameAnchor(wheelAnchor);
  assert.equal(await page.getByRole('button', {name: '新消息'}).count(), 0, 'non-message snapshots do not create unread badges');
  await draw(range(3, 102));
  await sameAnchor(wheelAnchor);
  await page.getByRole('button', {name: '新消息'}).click();
  await draw(range(2, 101));
  await readHistory();
  const before = await anchor();
  await draw(range(3, 102));
  await sameAnchor(before);
  assert.equal(await page.getByRole('button', {name: '新消息'}).count(), 1);
  await draw(range(6, 105));
  await sameAnchor(before);
  await page.getByRole('button', {name: '新消息'}).click();
  assert.ok(await gap() <= 1);
  assert.equal(await page.getByRole('button', {name: '新消息'}).count(), 0);
  await readHistory();
  await draw(range(7, 106));
  await page.locator('.messages').evaluate(n => { n.scrollTop = n.scrollHeight; n.dispatchEvent(new Event('scroll', {bubbles: true})); });
  assert.equal(await page.getByRole('button', {name: '新消息'}).count(), 0, 'manual bottom clears badge');
  await page.getByRole('button', {name: '搜索当前频道消息'}).click();
  await page.getByPlaceholder('搜索当前频道消息').fill('message 10');
  await draw(range(8, 107));
  assert.equal(await page.locator('.messages').evaluate(n => n.scrollTop), 0, 'new messages do not jump search');
  assert.equal(await page.getByRole('button', {name: '新消息'}).count(), 1);
  await page.getByRole('button', {name: '新消息'}).click();
  assert.equal(await page.getByPlaceholder('搜索当前频道消息').count(), 0);
  assert.ok(await gap() <= 1, 'jump exits search and reaches latest');
  await readHistory();
  await draw(range(1, 100), 'different-server');
  assert.ok(await gap() <= 1, 'same room ID on different server resets');
  await readHistory();
  await draw([], 'different-server', '43');
  await draw(range(1, 100), 'different-server', '43');
  assert.ok(await gap() <= 1, 'async room history follows initial load');
  await draw([], 'different-server', '44');
  await draw([1], 'different-server', '44');
  assert.equal(await page.getByRole('button', {name: '新消息'}).count(), 0, 'short histories do not show badge');
  await draw(range(1, 100));
  await readHistory();
  await draw(range(2, 101));
  await page.screenshot({path: join(tmpdir(), 'edenvoice-chat-scroll.png')});
  await draw(range(102, 201));
  assert.equal(await page.locator('.messages').evaluate(n => n.scrollTop), 0, 'fully expired anchor shows oldest remaining message');
  await page.getByRole('button', {name: '新消息'}).focus();
  await page.keyboard.press('Enter');
  assert.ok(await gap() <= 1, 'keyboard can jump to latest');
  await draw(range(1, 30), 'short-server');
  await readHistory();
  const shortAnchor = await anchor();
  await draw(range(1, 31), 'short-server');
  await sameAnchor(shortAnchor);
  await page.getByRole('button', {name: '新消息'}).click();
  await page.locator('.messages').evaluate(n => { n.scrollTop -= 20; n.dispatchEvent(new Event('scroll', {bubbles: true})); });
  await draw(range(1, 32), 'short-server');
  assert.ok(await gap() <= 1, 'within 32px of bottom keeps following');
  await page.locator('.message p').last().dblclick();
  assert.ok(await page.evaluate(()=>getSelection().toString().length>0),'chat text remains selectable');
  const composer=page.locator('.composer input:not([type="file"])');
  await composer.fill('editable text'); await composer.dblclick();
  assert.ok(await composer.evaluate(n=>n.selectionEnd>n.selectionStart),'input remains selectable');
  await page.evaluate(()=>{getSelection().removeAllRanges();document.getElementById('root').style.cssText='width:820px;height:700px';window.drawApp();});
  assert.equal(await page.locator('.member-panel').count(),0,'server member list starts collapsed');
  const channel=page.locator('.channel-row').first();await channel.waitFor();
  await channel.dblclick();
  assert.ok(await page.evaluate(()=>window.commands.some(c=>c?.type==='join_channel')),'double click still joins');
  assert.equal(await page.evaluate(()=>getSelection().toString()),'','room labels are not selected');
  const logo=page.locator('.titlebar img').first();
  assert.equal(await logo.evaluate(n=>n.draggable),false);
  assert.equal(await logo.evaluate(n=>n.dispatchEvent(new DragEvent('dragstart',{bubbles:true,cancelable:true}))),false,'logo drag is canceled');
  await page.setViewportSize({width:820,height:560});
  await page.waitForTimeout(100);
  assert.ok(await page.locator('.composer').evaluate(n=>n.getBoundingClientRect().bottom<=innerHeight-64),'composer stays above voice controls at minimum height');
  await page.screenshot({path:join(tmpdir(),'edenvoice-room-client.png')});
  assert.ok(await page.locator('.room-experience').evaluate(n=>n.scrollWidth<=n.clientWidth),'room fits minimum client width');
  assert.deepEqual(errors, []);
  console.log('PASS: bottom follow, anchored reading, rolling 100-message cap, batched arrivals, jump button, manual return, search, server/room switch, async history and short history.');
} finally {
  await browser?.close();
  await new Promise(resolve => server.close(resolve));
}
