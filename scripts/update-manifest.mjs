#!/usr/bin/env node
import {readFileSync,writeFileSync,existsSync} from 'node:fs';
import {basename} from 'node:path';
const [mode,version,platform,artifact,out]=process.argv.slice(2);
if(mode!=='create'||!/^\d+\.\d+\.\d+$/.test(version)||!/^(darwin|windows|linux)-(aarch64|x86_64)$/.test(platform||'')||!artifact||!out)throw Error('Usage: create VERSION PLATFORM ARTIFACT OUTPUT');
if(!existsSync(artifact))throw Error('Missing update artifact');
const signature=readFileSync(artifact+'.sig','utf8').trim();
if(!Buffer.from(signature,'base64').toString().startsWith('untrusted comment:'))throw Error('Invalid signature');
const base=process.env.EDENVOICE_COS_BASE_URL||'https://YOUR-BUCKET.cos.YOUR-REGION.myqcloud.com';
const url=`${base.replace(/\/$/,'')}/releases/v${version}/${platform}/${encodeURIComponent(basename(artifact))}`;
if(new URL(url).protocol!=='https:')throw Error('COS URL must use HTTPS');
writeFileSync(out,JSON.stringify({platform,version,url,signature,notes:`EdenVoice ${version}`},null,2)+'\n');
