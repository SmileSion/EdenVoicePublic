import assert from 'node:assert/strict';
import { test } from 'node:test';
import { appearanceKey, gameActivityLabel, seatPreview } from '../src/room-presentation.ts';
import type { User } from '../src/types.ts';

test('future activity rejects expired metadata and bounds display names', () => {
  const activity = {kind:'game' as const,title:'  Example  ',source:'windows' as const,updatedAt:10,expiresAt:30};
  assert.equal(gameActivityLabel(activity,20),'Example');
  assert.equal(gameActivityLabel(activity,30),null);
  assert.equal(gameActivityLabel(activity,9),null);
  assert.equal(gameActivityLabel({...activity,title:' '.repeat(5)},20),null);
  assert.equal(gameActivityLabel({...activity,title:'x'.repeat(100)},20)?.length,80);
});
test('seats stay bounded and stable across speaking changes without changing source order', () => {
  const users=Array.from({length:200},(_,i)=>({sessionId:String(i),speaking:false} as User));
  const before=users.map(u=>u.sessionId);
  const seats=seatPreview(users,'150','199');
  assert.equal(seats.length,5);
  assert.deepEqual(seats.slice(0,2).map(u=>u.sessionId),['150','199']);
  assert.deepEqual(seatPreview(users.map(u=>({...u,speaking:true})),'150','199').map(u=>u.sessionId),seats.map(u=>u.sessionId));
  assert.deepEqual(users.map(u=>u.sessionId),before);
  assert.notEqual(appearanceKey('a:b','c'),appearanceKey('a','b:c'));
});
