import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';
const { chromium } = createRequire(import.meta.url)(process.env.PLAYWRIGHT_MODULE || 'playwright');
const root = new URL('../product-site/', import.meta.url);
const release = JSON.parse(await readFile(new URL('releases.json', root), 'utf8'));
let config = release, httpStatus = 200;
const server = createServer(async (req, res) => {
  const path = new URL(req.url, 'http://localhost').pathname.replace(/^\/launch\//, '/');
  if (path === '/releases.json') {
    res.writeHead(httpStatus, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify(config)); return;
  }
  try {
    const name = path === '/' ? 'index.html' : path.slice(1);
    if (name.includes('..')) throw Error('path');
    const content = await readFile(new URL(name, root));
    res.setHeader('Content-Type', name.endsWith('.js') ? 'text/javascript' : name.endsWith('.css') ? 'text/css' : name.endsWith('.svg') ? 'image/svg+xml' : 'text/html; charset=utf-8');
    res.end(content);
  } catch { res.writeHead(404); res.end(); }
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const browser = await chromium.launch(process.env.CHROME_EXECUTABLE ? { executablePath: process.env.CHROME_EXECUTABLE } : {});
try {
  const page = await browser.newPage({ viewport: { width: 1280, height: 850 } });
  await page.emulateMedia({ reducedMotion: 'reduce' });
  const errors = []; page.on('pageerror', error => errors.push(error.message));
  const url = `http://127.0.0.1:${server.address().port}/launch/`;
  await page.goto(url);
  await page.waitForFunction(() => document.querySelectorAll('[data-download][href]').length === 5);
  for (const [key, value] of Object.entries(release.downloads)) assert.equal(await page.locator(`[data-download="${key}"]`).getAttribute('href'), value);
  assert.equal(await page.locator('[data-product-version]').first().textContent(), `v${release.product_version} · 功能介绍`);
  for (const [key, version] of Object.entries(release.versions)) assert.equal(await page.locator(`[data-release-platform="${key}"]`).textContent(), `${key.startsWith('server-') ? 'v' : '安装包 v'}${version}`);
  await page.locator('[data-mode="screen"]').click();
  assert.equal(await page.locator('#share-demo').isVisible(), true);
  assert.equal(await page.locator('.room-scene').isVisible(), false);
  await page.locator('#share-fullscreen').click();
  await page.waitForFunction(() => document.fullscreenElement?.id === 'shared-canvas');
  await page.evaluate(() => document.exitFullscreen());
  await page.locator('#experience').screenshot({path: '/tmp/edenvoice-085-screen-desktop.png', style: '.nav,.skip{visibility:hidden!important}'});
  await page.locator('#share-back').click();
  assert.equal(await page.locator('.room-scene').isVisible(), true);
  await page.locator('[data-mode="game"]').click();
  assert.equal(await page.locator('.room-scene').getAttribute('data-scene'), 'game');
  await page.locator('[data-mode="watch"]').click();
  assert.equal(await page.locator('.room-scene').getAttribute('data-scene'), 'watch');
  await page.locator('[data-memory-filter="game"]').click();
  assert.equal(await page.locator('[data-memory-kind]:visible').count(), 1);
  assert.equal(await page.locator('[data-memory-kind="game"]').isVisible(), true);
  await page.locator('[data-memory-filter="evening"]').click();
  assert.equal(await page.locator('[data-memory-kind="evening"]').isVisible(), true);
  await page.locator('[data-memory-filter="all"]').click();
  assert.equal(await page.locator('[data-memory-kind]:visible').count(), 2);
  await page.locator('#archive').screenshot({path: '/tmp/edenvoice-085-archive-desktop.png', style: '.nav,.skip{visibility:hidden!important}'});
  await page.locator('#download').scrollIntoViewIfNeeded();
  await page.screenshot({ path: '/tmp/edenvoice-085-downloads-desktop.png' });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.locator('#download').evaluate(el => el.scrollIntoView({ behavior: 'instant' }));
  await page.screenshot({ path: '/tmp/edenvoice-085-downloads-mobile.png' });
  assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
  await page.locator('[data-mode="screen"]').click();
  await page.locator('#experience').screenshot({path: '/tmp/edenvoice-085-screen-mobile.png', style: '.nav,.skip{visibility:hidden!important}'});
  await page.setViewportSize({width:320,height:740});
  assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
  await page.setViewportSize({width:390,height:844});
  assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
  // A package release can change independently of the product described on the page.
  config = {product_version:'0.8.5', version:'0.8.6', downloads: Object.fromEntries(Object.keys(release.downloads).map(key => [key, `https://downloads.example.invalid/v0.8.6/${key}`]))};
  await page.reload(); await page.getByText('安装包 v0.8.6', {exact:true}).first().waitFor();
  assert.equal(await page.locator('[data-product-version]').first().textContent(), 'v0.8.5 · 功能介绍');
  assert((await page.locator('[data-download]').first().getAttribute('href')).includes('0.8.6'));
  config = {...release, versions: {...release.versions, 'windows-x86_64':'<invalid>'}};
  await page.reload(); await page.locator('#download-retry').waitFor();
  assert.equal(await page.locator('[data-download][href]').count(), 0);
  config = { ...release, downloads: { ...release.downloads, 'linux-x86_64': null } };
  await page.reload(); await page.getByText('部分平台暂未提供下载。').waitFor();
  assert.equal(await page.locator('[data-download][href]').count(), 4);
  config = { ...release, downloads: { ...release.downloads, 'server-linux-arm64': 'javascript:alert(1)' } };
  await page.reload(); await page.locator('#download-retry').waitFor();
  assert.equal(await page.locator('[data-download][href]').count(), 0);
  httpStatus = 503; await page.reload(); await page.locator('#download-retry').waitFor();
  assert.equal(await page.locator('[data-download][href]').count(), 0);
  httpStatus = 200; config = release; await page.locator('#download-retry').click();
  await page.waitForFunction(() => document.querySelectorAll('[data-download][href]').length === 5);
  for (const name of ['guide.html','faq.html']) {
    await page.goto(url + name);
    assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
    assert.equal(await page.locator('#screen').count(), name === 'guide.html' ? 1 : 0);
  }
  assert.deepEqual(errors, []);
  console.log('PASS: actual per-platform versions, independent product version, screen preview/fullscreen, archive filters, subdirectory hosting, invalid manifest/URL, failure/retry and mobile pages.');
} finally { await browser.close(); server.close(); }
