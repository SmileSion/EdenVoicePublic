import { spawn, spawnSync } from 'node:child_process';
import { createServer } from 'node:net';
import { createRequire } from 'node:module';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { randomBytes } from 'node:crypto';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../..', import.meta.url));
const temp = mkdtempSync(join(tmpdir(), 'edenvoice-admin-test-'));
const executable = join(temp, process.platform === 'win32' ? 'admin-demo.exe' : 'admin-demo');
const env = { ...process.env, PLAYWRIGHT_MODULE: process.env.PLAYWRIGHT_MODULE || createRequire(import.meta.url).resolve('playwright') };
const tests = [
  'server/scripts/admin-ui.test.cjs', 'server/scripts/updates-ui.test.cjs',
  'desktop/scripts/announcements-admin.test.mjs',
  'desktop/scripts/client-build-admin.test.mjs', 'desktop/scripts/screen-share-admin.test.mjs',
];
let demo;
try {
  const build = spawnSync('go', ['build', '-o', executable, './cmd/admin-demo'], { cwd: join(root, 'server'), stdio: 'inherit' });
  if (build.error) throw build.error;
  if (build.status !== 0) throw new Error('admin-demo build failed');
  for (const file of tests) {
    const listener = createServer();
    await new Promise((resolve, reject) => { listener.once('error', reject); listener.listen(0, '127.0.0.1', resolve); });
    const port = listener.address().port;
    await new Promise(resolve => listener.close(resolve));
    const url = `http://127.0.0.1:${port}`;
    const testEnv = { ...env, EDENVOICE_DEMO_PASSWORD: randomBytes(24).toString('hex'), EDENVOICE_DEMO_ADDR: `127.0.0.1:${port}`, EDENVOICE_DEMO_URL: url, ADMIN_DEMO_URL: url, ADMIN_ARTIFACT_DIR: join(temp, 'screenshots') };
    demo = spawn(executable, [], { env: testEnv, stdio: ['ignore', 'ignore', 'inherit'] });
    demo.on('error', error => console.error(error));
    let ready = false;
    for (let i = 0; i < 100; i++) {
      if (demo.exitCode !== null) throw new Error('admin-demo exited before startup');
      try { ready = (await fetch(url, { signal: AbortSignal.timeout(500) })).ok; } catch {}
      if (ready) break;
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    if (!ready) throw new Error('admin-demo startup timed out');
    const code = await new Promise((resolve, reject) => {
      const child = spawn(process.execPath, [file], { cwd: root, env: testEnv, stdio: 'inherit' });
      const timer = setTimeout(() => { child.kill(); reject(new Error(`${file} timed out`)); }, 120_000);
      child.once('error', reject);
      child.once('exit', code => { clearTimeout(timer); resolve(code); });
    });
    await new Promise(resolve => { demo.once('exit', resolve); demo.kill(); });
    demo = undefined;
    if (code !== 0) throw new Error(`${file} failed`);
  }
} finally {
  demo?.kill();
  rmSync(temp, { recursive: true, force: true });
}
