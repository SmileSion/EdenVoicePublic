import {test} from 'node:test';
import assert from 'node:assert/strict';
import {applySnapshotPatch} from '../src/snapshotPatch.ts';
import type {CoreSnapshot} from '../src/types.ts';
test('audio deltas keep member/chat object identity and accept connection changes',()=>{
 const current={revision:5,users:[],messages:[],connection:'connected',inputLevel:0} as unknown as CoreSnapshot;
 const next=applySnapshotPatch(current,{baseRevision:5,revision:6,changes:{inputLevel:.5}})!;
 assert.equal(next.users,current.users);assert.equal(next.messages,current.messages);assert.equal(next.inputLevel,.5);assert.equal(current.inputLevel,0);
 assert.equal(applySnapshotPatch(next,{baseRevision:6,revision:7,changes:{connection:'disconnected'}})?.connection,'disconnected');
});
test('out of order patches cannot rewind state; a missing base requires resync',()=>{
 const current={revision:8} as CoreSnapshot;
 assert.equal(applySnapshotPatch(current,{baseRevision:6,revision:7,changes:{}}),current);
 assert.equal(applySnapshotPatch(current,{baseRevision:9,revision:10,changes:{}}),null);
});
