import assert from 'node:assert/strict';
import {mkdtempSync,readFileSync,rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {execFileSync} from 'node:child_process';
const dir=mkdtempSync(join(tmpdir(),'eden-update-manifest-'));
const run=(...args)=>execFileSync(process.execPath,['scripts/update-manifest.mjs',...args],{stdio:'pipe'});
try {
 const artifact='desktop/src-tauri/tests/fixtures/update.txt';
 run('create','0.8.0','darwin-aarch64',artifact,join(dir,'mac.json'));
 run('create','0.8.0','windows-x86_64',artifact,join(dir,'win.json'));
 const entry=JSON.parse(readFileSync(join(dir,'mac.json'),'utf8'));
 assert.equal(entry.platform,'darwin-aarch64');assert.equal(entry.version,'0.8.0');
 assert.match(entry.url,/releases\/v0.8.0\/darwin-aarch64\/update.txt$/);
 assert.ok(entry.signature.length>100);
 assert.throws(()=>run('create','../bad','windows-x86_64',artifact,join(dir,'bad.json')));
 console.log('PASS: COS admin descriptors and invalid version rejection');
} finally {rmSync(dir,{recursive:true,force:true});}
