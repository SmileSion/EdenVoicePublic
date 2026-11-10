import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { stripTypeScriptTypes } from 'node:module';
import test, { type TestContext } from 'node:test';
import type { RoomImageMetadata, RoomSocialContext, RoomSocialEvent, RoomSocialRequest } from '../src/types.ts';

// Node's native transform also handles the controller's constructor parameter
// properties; type-only React imports are erased without loading a DOM runtime.
const source = await readFile(new URL('../src/roomSocialController.ts', import.meta.url), 'utf8');
const { RoomSocialController, IMAGE_CHUNK_BYTES } = await import(`data:text/javascript;base64,${Buffer.from(stripTypeScriptTypes(source, { mode: 'transform' })).toString('base64')}`);
type Controller = import('../src/roomSocialController.ts').RoomSocialController;
type Request = (context: RoomSocialContext, request: RoomSocialRequest) => Promise<RoomSocialEvent>;

const context: RoomSocialContext = { serverAddress: 'example:7001', serverFingerprint: 'server-a', selfSessionId: '18446744073709551615', channelId: '3' };
const metadata = (reference = 'room-image:one', bytes = 4): RoomImageMetadata => ({ reference, mime: 'image/jpeg', width: 20, height: 10, bytes });
const jpeg = (length = 4) => { const bytes = new Uint8Array(length); bytes[0] = 0xff; bytes[1] = 0xd8; bytes[length - 2] = 0xff; bytes[length - 1] = 0xd9; return bytes; };
const response = (kind: RoomSocialEvent['kind'], fields: Partial<RoomSocialEvent> = {}): RoomSocialEvent => ({ kind, server_address: context.serverAddress, server_fingerprint: context.serverFingerprint, self_session_id: context.selfSessionId, channel_id: context.channelId, ...fields });
const deferred = <T>() => { let resolve!: (value: T) => void; let reject!: (reason: unknown) => void; const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const flush = async () => { for (let i = 0; i < 20; i++) await Promise.resolve(); };

function setup(t: TestContext, request: Request) {
  t.mock.timers.enable({ apis: ['Date', 'setTimeout', 'setInterval'], now: 1_000_000 });
  const revoked: string[] = [];
  let urls = 0;
  t.mock.method(URL, 'createObjectURL', () => `blob:test-${++urls}`);
  t.mock.method(URL, 'revokeObjectURL', (url: string) => { revoked.push(url); });
  const controller: Controller = new RoomSocialController(context, true, true, request);
  t.after(() => controller.dispose());
  return { controller, revoked };
}

async function advance(t: TestContext, milliseconds = 180) {
  await flush(); t.mock.timers.tick(milliseconds); await flush();
}

async function finish<T>(t: TestContext, promise: Promise<T>): Promise<T> {
  let done = false;
  let value!: T;
  let error: unknown;
  promise.then(result => { value = result; done = true; }, reason => { error = reason; done = true; });
  for (let i = 0; i < 1000 && !done; i++) await advance(t);
  assert.ok(done, 'operation must settle within the bounded simulated transfer lifetime');
  if (error) throw error;
  return value;
}

test('uploads use ordered 8 KiB chunks with one shared paced request lane', async t => {
  const bytes = jpeg(IMAGE_CHUNK_BYTES + 7);
  const calls: { request: RoomSocialRequest; at: number }[] = [];
  const { controller } = setup(t, async (_, request) => {
    calls.push({ request, at: Date.now() });
    if (request.action === 'image_begin') return response('image_upload', { upload_id: 'upload-a', offset: 0 });
    if (request.action === 'image_chunk') return response('image_upload', { upload_id: 'upload-a', offset: request.offset! + atob(request.data!).length });
    return response('image_committed', { image: metadata('room-image:one', bytes.length) });
  });
  await finish(t, controller.upload(bytes, 20, 10));
  assert.deepEqual(calls.map(item => item.request.action), ['image_begin', 'image_chunk', 'image_chunk', 'image_commit']);
  assert.deepEqual(calls.filter(item => item.request.action === 'image_chunk').map(item => [item.request.offset, atob(item.request.data!).length]), [[0, 8192], [8192, 7]]);
  for (let i = 1; i < calls.length; i++) assert.ok(calls[i].at - calls[i - 1].at >= 180);
  assert.equal(controller.snapshot().upload, null);
  assert.ok(!JSON.stringify(controller.snapshot()).includes('data'));
});

test('a mismatched upload acknowledgment cancels the upload and never commits', async t => {
  const actions: string[] = [];
  const { controller } = setup(t, async (_, request) => {
    actions.push(request.action);
    if (request.action === 'image_begin') return response('image_upload', { upload_id: 'upload-a' });
    if (request.action === 'image_chunk') return response('image_upload', { upload_id: 'upload-a', offset: 1 });
    return response('image_cancelled');
  });
  await assert.rejects(finish(t, controller.upload(jpeg(), 20, 10)), /进度不一致/);
  assert.deepEqual(actions, ['image_begin', 'image_chunk', 'image_cancel']);
  assert.equal(controller.snapshot().upload, null);
});

test('busy image responses retry the identical unaccepted chunk after the server delay', async t => {
  const chunks: { request: RoomSocialRequest; at: number }[] = [];
  const { controller } = setup(t, async (_, request) => {
    if (request.action === 'image_begin') return response('image_upload', { upload_id: 'upload-a' });
    if (request.action === 'image_chunk') {
      chunks.push({ request, at: Date.now() });
      if (chunks.length === 1) throw Object.assign(new Error('busy'), { code: 'busy', retryAfterMs: 400 });
      return response('image_upload', { upload_id: 'upload-a', offset: 4 });
    }
    return response('image_committed', { image: metadata() });
  });
  await finish(t, controller.upload(jpeg(), 20, 10));
  assert.equal(chunks.length, 2);
  assert.deepEqual(chunks[0].request, chunks[1].request);
  assert.ok(chunks[1].at - chunks[0].at >= 400);
});

test('canceling an upload awaiting begin cancels the returned reservation without sending bytes', async t => {
  const begin = deferred<RoomSocialEvent>();
  const actions: string[] = [];
  const { controller } = setup(t, async (_, request) => {
    actions.push(request.action);
    return request.action === 'image_begin' ? begin.promise : response('image_cancelled');
  });
  const upload = controller.upload(jpeg(), 20, 10);
  await advance(t);
  controller.cancelUpload();
  begin.resolve(response('image_upload', { upload_id: 'late-upload' }));
  await assert.rejects(finish(t, upload), /取消/);
  assert.ok(actions.includes('image_cancel'));
  assert.ok(!actions.includes('image_chunk'));
});

test('disposing while begin is in flight also releases the returned reservation', async t => {
  const begin = deferred<RoomSocialEvent>();
  const actions: string[] = [];
  const { controller } = setup(t, async (_, request) => {
    actions.push(request.action);
    return request.action === 'image_begin' ? begin.promise : response('image_cancelled');
  });
  const upload = controller.upload(jpeg(), 20, 10);
  await advance(t);
  controller.dispose();
  begin.resolve(response('image_upload', { upload_id: 'late-upload' }));
  await assert.rejects(finish(t, upload), /取消/);
  assert.ok(actions.includes('image_cancel'), 'a late image_begin must release its server reservation');
  assert.ok(!actions.includes('image_chunk'));
});

for (const failure of ['oversized', 'wrong_offset', 'early_eof', 'invalid_base64']) {
  test(`image downloads reject ${failure} chunks without caching partial bytes`, async t => {
    let calls = 0;
    const { controller } = setup(t, async (_, request) => {
      calls++;
      const fields: Partial<RoomSocialEvent> = { reference: request.reference, offset: request.offset, eof: false, data: Buffer.from(jpeg()).toString('base64') };
      if (failure === 'oversized') fields.data = Buffer.from(jpeg(8193)).toString('base64');
      if (failure === 'wrong_offset') fields.offset = 1;
      if (failure === 'early_eof') fields.eof = true;
      if (failure === 'invalid_base64') fields.data = '@@@@';
      return response('image_data', fields);
    });
    await assert.rejects(finish(t, controller.image(metadata('room-image:bad', 10000))));
    await assert.rejects(finish(t, controller.image(metadata('room-image:bad', 10000))));
    assert.equal(calls, 2, 'failed image bodies must not enter the cache');
  });
}

test('only eight downloads queue, one reaches the server at a time, and disposal invalidates them all', async t => {
  const first = deferred<RoomSocialEvent>();
  let calls = 0;
  const { controller } = setup(t, async () => { calls++; return first.promise; });
  const downloads = Array.from({ length: 8 }, (_, i) => controller.image(metadata(`room-image:${i}`)));
  const results = Promise.allSettled(downloads);
  await assert.rejects(controller.image(metadata('room-image:ninth')), /图片较多/);
  await advance(t);
  assert.equal(calls, 1);
  controller.dispose();
  first.resolve(response('image_data', { reference: 'room-image:0', offset: 0, eof: true, data: Buffer.from(jpeg()).toString('base64') }));
  assert.ok((await finish(t, results)).every(result => result.status === 'rejected'));
  assert.equal(calls, 1);
});

test('room, server, session, and server identity mismatches cannot change presence', t => {
  const { controller } = setup(t, async () => response('status'));
  for (const fields of [{ channel_id: '4' }, { server_address: 'other:7001' }, { self_session_id: '8' }, { server_fingerprint: 'server-b' }]) {
    controller.accept(response('away', { session_id: '9', active: true, ...fields }));
  }
  assert.deepEqual(controller.snapshot().away, {});
  controller.accept(response('away', { session_id: '9', active: true }));
  assert.ok(controller.snapshot().away['9']);
});

test('reactions deduplicate, remain bounded, and expire without an audio snapshot', async t => {
  const { controller } = setup(t, async () => response('status'));
  for (let i = 0; i < 40; i++) {
    const reaction = response('reaction', { session_id: String(i), emoji: '👍', expires_at_unix_ms: Date.now() + 2500 });
    controller.accept(reaction); controller.accept(reaction);
  }
  assert.equal(controller.snapshot().reactions.length, 24);
  await advance(t, 3000);
  assert.equal(controller.snapshot().reactions.length, 0);
});

test('cached URLs are scoped to a controller and released on disposal', async t => {
  let calls = 0;
  const { controller, revoked } = setup(t, async (_, request) => { calls++; return response('image_data', { reference: request.reference, offset: request.offset, eof: true, data: Buffer.from(jpeg()).toString('base64') }); });
  const image = metadata();
  const url = await finish(t, controller.image(image));
  assert.equal(await controller.image(image), url);
  assert.equal(calls, 1);
  controller.dispose();
  assert.deepEqual(revoked, [url]);
  await assert.rejects(controller.image(image), /取消/);
});

test('late status RPC completion cannot overwrite a newer ordered presence event', async t => {
  const status = deferred<RoomSocialEvent>();
  const { controller } = setup(t, async () => status.promise);
  const refresh = controller.refresh();
  controller.accept(response('status', { away: [] }));
  controller.accept(response('away', { session_id: '9', active: true }));
  status.resolve(response('status', { away: [] }));
  await finish(t, refresh);
  assert.ok(controller.snapshot().away['9'], 'state changes come from the ordered event listener, never duplicated RPC completion');
});

test('cache capacity is bounded and eviction notifies visible attachments before reload', async t => {
  const { controller, revoked } = setup(t, async (_, request) => response('image_data', { reference: request.reference, offset: request.offset, eof: true, data: Buffer.from(jpeg()).toString('base64') }));
  let evictions = 0;
  const stop = controller.watchImage('room-image:0', () => { evictions++; });
  t.after(stop);
  const urls: string[] = [];
  for (let i = 0; i < 33; i++) urls.push(await finish(t, controller.image(metadata(`room-image:${i}`))));
  assert.deepEqual(revoked, [urls[0]]);
  assert.equal(evictions, 1);
  const reloaded = await finish(t, controller.image(metadata('room-image:0')));
  assert.notEqual(reloaded, urls[0]);
  controller.dispose();
  assert.equal(new Set(revoked).size, 34, 'evicted and resident URL allocations must all be revoked exactly once');
});

test('overlapping upload and download requests share one paced lane', async t => {
  let inFlight = 0;
  let peak = 0;
  const started: number[] = [];
  const { controller } = setup(t, async (_, request) => {
    inFlight++; peak = Math.max(peak, inFlight); started.push(Date.now());
    await Promise.resolve();
    inFlight--;
    if (request.action === 'image_begin') return response('image_upload', { upload_id: 'upload-a' });
    if (request.action === 'image_chunk') return response('image_upload', { upload_id: 'upload-a', offset: 4 });
    if (request.action === 'image_get') return response('image_data', { reference: request.reference, offset: 0, eof: true, data: Buffer.from(jpeg()).toString('base64') });
    return response('image_committed', { image: metadata('room-image:uploaded') });
  });
  await finish(t, Promise.all([controller.upload(jpeg(), 20, 10), controller.image(metadata('room-image:download'))]));
  assert.equal(peak, 1);
  for (let i = 1; i < started.length; i++) assert.ok(started[i] - started[i - 1] >= 180);
});


test('away remains until an explicit return event, independently from reaction expiry', async t => {
  const { controller } = setup(t, async () => response('status'));
  controller.accept(response('away', { session_id: '9', active: true }));
  await advance(t, 24 * 60 * 60 * 1000);
  assert.ok(controller.snapshot().away['9']);
  controller.accept(response('away', { session_id: '9', active: false }));
  assert.deepEqual(controller.snapshot().away, {});
});
