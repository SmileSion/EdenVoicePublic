import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { fileURLToPath } from 'node:url';
import { build } from 'esbuild';

const { chromium } = createRequire(import.meta.url)(process.env.PLAYWRIGHT_MODULE || 'playwright');
const root = fileURLToPath(new URL('..', import.meta.url));
const bundle = await build({
  stdin: { resolveDir: root, loader: 'tsx', contents: `
    import React,{useState} from 'react';import{createRoot}from'react-dom/client';
    import{RoomExperience}from'./src/WatchParty';import{roomPresence}from'./src/RoomPresence';import{mockSnapshot}from'./src/mock';
    window.calls=[];window.helpers={roomPresence};
    function Harness(){
      const [room,setRoom]=useState('room'),[theme,setTheme]=useState('dark'),[count,setCount]=useState(60);
      const [social,setSocial]=useState({supported:true,away:{'member-0':{sessionId:'member-0'},departed:{sessionId:'departed'}},reactions:[],busy:false,error:''});
      window.patchSocial=(next)=>setSocial(previous=>({...previous,...next}));window.changeRoom=setRoom;window.changeTheme=setTheme;window.changeCount=setCount;
      const snapshot={...mockSnapshot,selfSessionId:'member-0',theme,language:'chinese',users:Array.from({length:count},(_,i)=>({...mockSnapshot.users[0],sessionId:'member-'+i,clientId:'client-'+i,nickname:i===0?'你':'朋友 '+i,channelId:'room',speaking:false,muted:false,adminMuted:false})),messages:[{id:'speech',channelId:'room',senderSessionId:'member-0',senderName:'你',text:'这里的消息仍然清楚可读',sentAtUnixMs:Date.now()}]};
      return <div className={'theme-'+theme}><RoomExperience snapshot={snapshot} channelId={room} visible onReveal={()=>{}} social={social} onAway={active=>{window.calls.push(['away',active]);setSocial(previous=>({...previous,away:active?{'member-0':{sessionId:'member-0'}}:{}}));}} onReaction={emoji=>{window.calls.push(['reaction',emoji]);setSocial(previous=>({...previous,reactions:[...previous.reactions,{id:String(Date.now()),sessionId:'member-0',emoji,expiresAtUnixMs:Date.now()+2500}]}));}}/></div>;
    }createRoot(document.getElementById('root')).render(<Harness/>);
  ` },
  bundle: true, write: false, format: 'iife', jsx: 'automatic', loader: { '.svg': 'text' },
  plugins: [{ name: 'isolated-room', setup(b) {
    b.onResolve({ filter: /^\.\/(useWatchRoom|Avatar|FoyerAtmosphere)$/ }, args => ({ path: args.path, namespace: 'mock' }));
    b.onLoad({ filter: /.*/, namespace: 'mock' }, args => ({ resolveDir: root, loader: 'tsx', contents: args.path.endsWith('useWatchRoom') ? `export function useWatchRoom(){return {view:{members:[],hostId:'',roomOpen:false,active:false},error:'',busy:false,available:true,resume:null};}` : args.path.endsWith('Avatar') ? `export function Avatar({name}){return <span className="avatar" aria-label={name}>友</span>}` : `export function FoyerAtmosphere(){return null}` }));
  } }],
});
const css = (await Promise.all(['styles.css', 'room-experience.css'].map(name => readFile(root + '/src/' + name, 'utf8')))).join('\n');
const server = createServer((request, response) => {
  response.setHeader('Content-Type', request.url === '/app.js' ? 'text/javascript' : request.url === '/style.css' ? 'text/css' : 'text/html');
  response.end(request.url === '/app.js' ? bundle.outputFiles[0].text : request.url === '/style.css' ? css : '<html><meta charset="UTF-8"><link rel="stylesheet" href="/style.css"><style>body{margin:0}#root{width:100%;height:100vh}#root>div{height:100%;background:var(--panel)}.room-experience{height:100%}</style><div id="root"></div><script src="/app.js"></script></html>');
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const browser = await chromium.launch({ executablePath: process.env.CHROME_EXECUTABLE });
try {
  const page = await browser.newPage({ viewport: { width: 740, height: 780 } });
  const errors = []; page.on('pageerror', error => errors.push(String(error)));
  await page.goto('http://127.0.0.1:' + server.address().port);
  await page.locator('.room-social-actions').waitFor();
  assert.equal(await page.locator('.friend-seat>.room-away-badge').count(), 1, 'departed away states stay hidden');
  assert.match(await page.locator('[data-seat-session="member-0"]>small').textContent(), /暂离/, 'seat status reflects persistent away');
  await page.getByRole('button', { name: '返回并开麦', exact: true }).click();
  await page.getByRole('button', { name: '暂离并关闭麦克风', exact: true }).click();
  assert.deepEqual(await page.evaluate(() => window.calls.at(-1)), ['away', true]);
  assert.equal(await page.locator('.room-away-options').count(),0,'no duration selection');
  await page.getByRole('button', { name: '返回并开麦', exact: true }).click();
  assert.equal(await page.locator('.room-away-notice').count(), 0);
  assert.deepEqual(await page.evaluate(() => window.calls.at(-1)), ['away', false]);
  await page.getByRole('button', { name: '发送表情回应', exact: true }).click();
  assert.equal(await page.locator('.room-reaction-options button').count(), 6);
  await page.keyboard.press('Escape');
  assert.equal(await page.locator('.room-reaction-options').count(), 0);
  assert.equal(await page.getByRole('button', { name: '发送表情回应', exact: true }).evaluate(button => button === document.activeElement), true);
  await page.getByRole('button', { name: '发送表情回应', exact: true }).click();
  await page.getByRole('button', { name: '赞', exact: true }).click();
  assert.equal(await page.locator('[data-seat-session="member-0"] .seat-reaction').textContent(), '👍');
  await page.evaluate(() => window.patchSocial({ reactions: [{ id: 'old', sessionId: 'member-0', emoji: '👍', expiresAtUnixMs: Date.now()+2200 }, { id: 'new', sessionId: 'member-0', emoji: '❤️', expiresAtUnixMs: Date.now()+2500 }] }));
  await page.waitForFunction(() => document.querySelector('.seat-reaction')?.textContent === '❤️');
  assert.equal(await page.locator('.seat-reaction').count(), 1, 'new emoji replaces old emoji for the same seat');
  const placement = await page.locator('[data-seat-session="member-0"]').evaluate(seat => {
    const reaction = seat.querySelector('.seat-reaction').getBoundingClientRect(), message = seat.querySelector('.seat-messages').getBoundingClientRect();
    return reaction.top >= message.bottom;
  });
  assert(placement, 'reaction stays below speech bubble');
  await page.evaluate(() => window.patchSocial({ reactions: Array.from({length:5},(_,i)=>({id:'side-'+i,sessionId:'member-'+(55+i),emoji:'👏',expiresAtUnixMs:Date.now()+2500})) }));
  await page.locator('.room-side-reaction').first().waitFor();
  assert.equal(await page.locator('.room-side-reaction').count(), 3);
  assert.match(await page.locator('.room-side-reactions').textContent(), /朋友 59/);
  await page.screenshot({path:'/tmp/edenvoice-room-presence-desktop.png'});
  await page.evaluate(() => window.changeTheme('light'));
  await page.setViewportSize({width:320,height:640});
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
  await page.getByRole('button', {name:'发送表情回应',exact:true}).click();
  const popover=await page.locator('.room-reaction-options').boundingBox();
  assert(popover.x>=0&&popover.x+popover.width<=320,'popover fits narrow room');
  await page.keyboard.press('Escape');
  await page.emulateMedia({reducedMotion:'reduce'});
  await page.evaluate(() => window.patchSocial({ reactions: [{id:'motion',sessionId:'member-0',emoji:'☕',expiresAtUnixMs:Date.now()+2500}] }));
  assert.equal(await page.locator('.seat-reaction').evaluate(el=>getComputedStyle(el).animationName),'none');
  await page.screenshot({path:'/tmp/edenvoice-room-presence-mobile.png'});
  await page.locator('.seat-reaction').waitFor({state:'detached',timeout:4000});
  await page.evaluate(() => window.patchSocial({away:{'member-0':{sessionId:'member-0'}}}));
  await page.locator('.room-away-notice').waitFor();
  await page.getByRole('button',{name:'返回并开麦',exact:true}).click();
  await page.locator('.room-away-notice').waitFor({state:'detached',timeout:2000});
  await page.evaluate(() => {window.patchSocial({reactions:[{id:'switch',sessionId:'member-0',emoji:'🎉',expiresAtUnixMs:Date.now()+2500}]});window.changeRoom('other');});
  await page.waitForFunction(()=>!document.querySelector('.seat-reaction,.room-side-reaction,.room-social-actions'));
  assert.equal(await page.locator('.seat-reaction,.room-side-reaction,.room-social-actions').count(),0,'no stale room activity after changing rooms');
  await page.evaluate(() => window.changeRoom('0'));
  assert.equal(await page.locator('.room-social-actions,.seat-reaction,.room-away-badge').count(),0,'foyer has no social controls or room states');
  assert.deepEqual(errors, []);
  console.log('PASS: away/return callbacks, expiry/member filtering, latest reaction, bounded side activity, speech clearance, 320px layout, reduced motion and room/foyer isolation');
} finally { await browser.close(); server.close(); }
