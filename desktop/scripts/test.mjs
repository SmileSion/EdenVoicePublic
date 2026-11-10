import { readdirSync } from 'node:fs';
import { createRequire } from 'node:module';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const desktop = fileURLToPath(new URL('..', import.meta.url));
const unit = readdirSync(new URL('.', import.meta.url))
  .filter(name => name.endsWith('.test.ts'))
  .map(name => `scripts/${name}`)
  .concat(['screen-share.test.mjs', 'snapshot-bridge.test.mjs'].map(name => `scripts/${name}`));
// These tests create and close their own loopback fixtures. Standalone visual
// demos are deliberately run via their separate launchers.
const browser = [
  'announcements', 'archive-recap', 'chat-scroll', 'close-behavior',
  'daily-features', 'experience-ui', 'focus-timer', 'member-panel',
  'muted-speech', 'recovery', 'room-presence',
  'screen-share-ui', 'server-invitation-ui',
  'system-audio-worklet', 'voice-incident', 'watch-party',
].map(name => `scripts/${name}.test.${name === 'screen-share-tabs' ? 'cjs' : 'mjs'}`);
const mode = process.argv[2] || 'all';
if (!['all', 'unit', 'browser'].includes(mode)) throw new Error('Use all, unit, or browser');
const files = mode === 'unit' ? unit : mode === 'browser' ? browser : [...unit, ...browser];
const result = spawnSync(process.execPath, ['--test', '--test-concurrency=1', ...files], {
  cwd: desktop, stdio: 'inherit',
  env: { ...process.env, PLAYWRIGHT_MODULE: process.env.PLAYWRIGHT_MODULE || createRequire(import.meta.url).resolve('playwright') },
  timeout: 600_000,
});
if (result.error) throw result.error;
process.exitCode = result.status ?? 1;
