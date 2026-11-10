import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { createServer } from 'node:http';
import { fileURLToPath } from 'node:url';
import { build } from 'esbuild';

const { chromium } = createRequire(import.meta.url)(process.env.PLAYWRIGHT_MODULE || 'playwright');
const root = fileURLToPath(new URL('..', import.meta.url));
const bundle = await build({ stdin: { resolveDir: root, loader: 'tsx', contents: `
import React,{StrictMode,useState} from 'react';import{createRoot}from'react-dom/client';
import{MutedSpeechNotice}from'./src/MutedSpeechNotice';import{mockSnapshot}from'./src/mock';
window.commands=[];
const initial={...mockSnapshot,connection:'connected',serverAddress:'local-test',selfSessionId:'self',muted:true,deafened:false,selfAway:false,audio:{...mockSnapshot.audio,inputMode:'voice_activity'},users:[{...mockSnapshot.users[0],sessionId:'self',channelId:'room',adminMuted:false}],mutedSpeechNotice:{sequence:'1',atUnixMs:Date.now()}};
function Harness(){const[snapshot,setSnapshot]=useState(initial);window.patch=value=>setSnapshot(old=>({...old,...value}));window.reset=()=>setSnapshot({...initial,mutedSpeechNotice:null});return <MutedSpeechNotice snapshot={snapshot}/>}
createRoot(document.getElementById('root')).render(<StrictMode><Harness/></StrictMode>);
` }, bundle: true, write: false, outdir: '/tmp/edenvoice-muted-notice-test', format: 'iife', jsx: 'automatic',
  plugins: [{ name: 'isolated-notice', setup(b) {
    b.onResolve({ filter: /^\.\/bridge$/ }, () => ({ path: 'bridge', namespace: 'mock' }));
    b.onLoad({ filter: /.*/, namespace: 'mock' }, () => ({ contents: `export async function dispatch(command){window.commands.push(command);window.patch({muted:command.muted});}`, loader: 'js' }));
  } }],
});
const js = bundle.outputFiles.find(file => file.path.endsWith('.js')).text;
const server = createServer((req, res) => {
  res.setHeader('Content-Type', req.url === '/app.js' ? 'text/javascript' : 'text/html');
  res.end(req.url === '/app.js' ? js : '<html><meta charset="UTF-8"><div id="root"></div><script src="/app.js"></script></html>');
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const browser = await chromium.launch({ executablePath: process.env.CHROME_EXECUTABLE });
try {
  const page = await browser.newPage();
  const errors = []; page.on('pageerror', e => errors.push(String(e)));
  await page.clock.install();
  await page.goto('http://127.0.0.1:' + server.address().port);
  await page.locator('.muted-speech-notice').waitFor();
  await page.clock.fastForward(8001);
  await page.locator('.muted-speech-notice').waitFor({ state: 'detached' });
  const show = async sequence => {
    await page.evaluate(sequence => window.patch({ mutedSpeechNotice: { sequence, atUnixMs: Date.now() } }), sequence);
  };
  await show('2'); await page.locator('.muted-speech-notice').waitFor();
  await page.getByRole('button', { name: '关闭静音说话提醒' }).click();
  await page.evaluate(() => window.patch({ outputLevel: 1 }));
  assert.equal(await page.locator('.muted-speech-notice').count(), 0);
  await show('3'); await page.locator('.muted-speech-notice').waitFor();
  await page.evaluate(() => window.patch({ users: [{ sessionId: 'self', channelId: 'other', adminMuted: false }] }));
  await page.locator('.muted-speech-notice').waitFor({ state: 'detached' });
  await show('4'); await page.locator('.muted-speech-notice').waitFor();
  await page.getByRole('button', { name: '打开麦克风' }).click();
  await page.locator('.muted-speech-notice').waitFor({ state: 'detached' });
  assert.deepEqual(await page.evaluate(() => window.commands), [{ type: 'set_muted', muted: false }]);
  for (const patch of [{ selfAway: true }, { deafened: true }, { connection: 'reconnecting' }, { users: [{ sessionId: 'self', channelId: '0' }] }, { users: [{ sessionId: 'self', channelId: 'room', adminMuted: true }] }]) {
    await page.evaluate(patch => { window.reset(); window.patch({ ...patch, mutedSpeechNotice: { sequence: crypto.randomUUID(), atUnixMs: Date.now() } }); }, patch);
    await page.clock.runFor(16);
    assert.equal(await page.locator('.muted-speech-notice').count(), 0, JSON.stringify(patch));
  }
  await page.evaluate(() => { window.reset(); window.patch({ mutedSpeechNotice: { sequence: 'old', atUnixMs: Date.now() - 9000 } }); });
  await page.clock.runFor(16);
  assert.equal(await page.locator('.muted-speech-notice').count(), 0);
  assert.deepEqual(errors, []);
  console.log('PASS: StrictMode initial-event expiry, dismissal dedup, room change invalidation, explicit unmute, away/deafen/admin/lobby suppression, stale events');
} finally { await browser.close(); server.close(); }
