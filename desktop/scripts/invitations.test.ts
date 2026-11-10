import {test} from 'node:test';
import assert from 'node:assert/strict';
import {createInvitation,parseInvitation} from '../src/invitations.ts';
test('invitation round trip preserves server, never carries password',()=>{for(const address of ['voice.example.com:7000','192.168.1.5:9000','[::1]:7000'])assert.equal(parseInvitation(createInvitation(address)),address);});
test('reject credentials, duplicate or unknown fields, bad ports and injected paths',()=>{for(const value of ['edenvoice://join?address=x:7000&password=secret','edenvoice://join?address=x:7000&address=y:7000','edenvoice://join?address=x:0','edenvoice://join?address=x:65536','edenvoice://join?address=x%0A:7000','edenvoice://join?address=x/path:7000','https://join?address=x:7000','edenvoice://join?address=x:7000#evil'])assert.throws(()=>parseInvitation(value));});
