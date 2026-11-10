import type { RoomSocialContext, RoomSocialEvent, RoomSocialRequest, ChatMessage } from './types';
import type { RoomSocialView } from './RoomPresence';

export type RoomImageMeta = NonNullable<ChatMessage['image']>;
type Request = (context: RoomSocialContext, request: RoomSocialRequest) => Promise<RoomSocialEvent>;
type ImageProgress = { sent: number; total: number } | null;
export const IMAGE_CHUNK_BYTES = 8192;
export const MAX_ROOM_IMAGE_BYTES = 256 * 1024;
export const ROOM_REACTIONS = ['👍', '❤️', '😂', '🎉', '👏', '☕'] as const;
const delay = (ms: number) => new Promise<void>(resolve => setTimeout(resolve, ms));
const canceled = () => new Error('房间已改变或操作已取消');

export function bytesToBase64(bytes: Uint8Array): string {
  let value = '';
  for (const byte of bytes) value += String.fromCharCode(byte);
  return btoa(value);
}

/** Social/image work is bounded and independent from the audio state snapshot. */
export class RoomSocialController {
  private disposed = false;
  private listeners = new Set<() => void>();
  private uploadGeneration = 0;
  private uploadId = '';
  private reactionAt = 0;
  private expiryTimer: ReturnType<typeof setInterval>;
  private downloads = new Map<string, Promise<string>>();
  private imageCache = new Map<string, { url: string; bytes: number }>();
  private imageEvictions = new Map<string, Set<() => void>>();
  private downloadTail: Promise<unknown> = Promise.resolve();
  private nextImageAt = 0;
  private transferTail: Promise<unknown> = Promise.resolve();
  private view: RoomSocialView & { upload: ImageProgress };

  constructor(readonly context: RoomSocialContext, readonly imagesSupported: boolean, supported: boolean, private request: Request) {
    this.view = { supported, away: {}, reactions: [], busy: false, error: '', upload: null };
    this.expiryTimer = setInterval(() => this.expire(), 250);
  }
  snapshot = () => this.view;
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  private patch(change: Partial<typeof this.view>) {
    if (this.disposed) return;
    this.view = { ...this.view, ...change };
    this.listeners.forEach(listener => listener());
  }
  private valid() { if (this.disposed) throw canceled(); }
  private async call(request: Omit<RoomSocialRequest, 'channel_id'>) {
    this.valid();
    const response = await this.request(this.context, { ...request, channel_id: this.context.channelId });
    if (request.action !== 'image_begin') this.valid();
    return response;
  }
  async refresh() {
    if (!this.view.supported) return;
    try { await this.call({ action: 'status' }); }
    catch (error) { if (!this.disposed) this.patch({ error: error instanceof Error ? error.message : '无法读取房间状态' }); }
  }
  accept(event: RoomSocialEvent) {
    if (this.disposed || event.channel_id !== this.context.channelId || event.server_address !== this.context.serverAddress || event.self_session_id !== this.context.selfSessionId) return;
    if (this.context.serverFingerprint && event.server_fingerprint !== this.context.serverFingerprint) return;
    const now = Date.now();
    if (event.kind === 'status') {
      const away = Object.fromEntries((event.away ?? []).map(item => [item.session_id, { sessionId: item.session_id }]));
      this.patch({ away });
    } else if (event.kind === 'away' && event.session_id) {
      const away = { ...this.view.away };
      if (event.active === true) away[event.session_id] = { sessionId: event.session_id };
      else delete away[event.session_id];
      this.patch({ away });
    } else if (event.kind === 'reaction' && event.session_id && ROOM_REACTIONS.includes(event.emoji as typeof ROOM_REACTIONS[number]) && (event.expires_at_unix_ms ?? 0) > now) {
      const expiresAtUnixMs = Math.min(event.expires_at_unix_ms!, now + 2500);
      const reaction = { id: `${event.session_id}:${event.expires_at_unix_ms}:${event.emoji}`, sessionId: event.session_id, emoji: event.emoji!, expiresAtUnixMs };
      if (this.view.reactions.some(item => item.id === reaction.id)) return;
      this.patch({ reactions: [...this.view.reactions.filter(item => item.sessionId !== event.session_id && item.expiresAtUnixMs > now), reaction].slice(-24) });
    }
  }
  private expire() {
    if (this.disposed) return;
    const now = Date.now();
    const reactions = this.view.reactions.filter(item => item.expiresAtUnixMs > now);
    if (reactions.length !== this.view.reactions.length) this.patch({ reactions });
  }
  async setAway(active: boolean) {
    if (!this.view.supported || this.view.busy) return;
    this.patch({ busy: true, error: '' });
    try { await this.call({ action: 'away', active }); }
    catch (error) { this.patch({ error: error instanceof Error ? error.message : '状态设置失败，请重试' }); }
    finally { this.patch({ busy: false }); }
  }
  async react(emoji: string) {
    if (!this.view.supported || !ROOM_REACTIONS.includes(emoji as typeof ROOM_REACTIONS[number])) return;
    if (Date.now() - this.reactionAt < 2000) { this.patch({ error: '等两秒，再回应一次吧' }); return; }
    this.reactionAt = Date.now();
    this.patch({ error: '' });
    try { await this.call({ action: 'reaction', emoji }); }
    catch (error) { this.patch({ error: error instanceof Error ? error.message : '回应发送失败，请重试' }); }
  }
  /** A shared queue bounds image traffic even when upload and downloads overlap. */
  private imageCall(request: Omit<RoomSocialRequest, 'channel_id'>, check: () => void = () => this.valid()) {
    const task = this.transferTail.catch(() => {}).then(async () => {
      const deadline = Date.now() + 60_000;
      for (;;) {
        check(); this.valid();
        await delay(Math.max(0, this.nextImageAt - Date.now()));
        check(); this.valid();
        this.nextImageAt = Date.now() + 180;
        try { return await this.call(request); }
        catch (error) {
          const retry = Number((error as { retryAfterMs?: number }).retryAfterMs);
          if (!retry || retry < 0 || Date.now() + retry > deadline) throw error;
          await delay(Math.min(5000, Math.max(180, retry)));
        }
      }
    });
    this.transferTail = task;
    return task;
  }
  async upload(bytes: Uint8Array, width: number, height: number) {
    if (!this.imagesSupported) throw new Error('服务器尚未支持图片消息');
    if (this.view.upload) throw new Error('请等待当前图片发送完成');
    if (!bytes.length || bytes.length > MAX_ROOM_IMAGE_BYTES || width < 1 || height < 1 || width > 1600 || height > 1600) throw new Error('图片尺寸或大小超出限制');
    const generation = ++this.uploadGeneration;
    const check = () => { this.valid(); if (generation !== this.uploadGeneration) throw canceled(); };
    this.patch({ upload: { sent: 0, total: bytes.length }, error: '' });
    let id = '';
    try {
      const begin = await this.imageCall({ action: 'image_begin', bytes: bytes.length, width, height }, check);
      id = begin.upload_id ?? '';
      if (!id) throw new Error('服务器没有创建图片上传，请重试');
      if (generation !== this.uploadGeneration || this.disposed) {
        void this.request(this.context, { channel_id: this.context.channelId, action: 'image_cancel', upload_id: id }).catch(() => {});
        throw canceled();
      }
      this.uploadId = id;
      for (let offset = 0; offset < bytes.length;) {
        check();
        const part = bytes.subarray(offset, offset + IMAGE_CHUNK_BYTES);
        const reply = await this.imageCall({ action: 'image_chunk', upload_id: id, offset, data: bytesToBase64(part) }, check);
        check();
        offset += part.length;
        if (reply.offset !== offset) throw new Error('图片传输进度不一致，请重试');
        this.patch({ upload: { sent: offset, total: bytes.length } });
      }
      check();
      const result = await this.imageCall({ action: 'image_commit', upload_id: id }, check);
      check();
      if (!result.image) throw new Error('图片发送结果不完整，请检查聊天记录');
      this.remember(result.image, bytes);
      this.uploadId = '';
      return result.image;
    } catch (error) {
      if (id) void this.request(this.context, { channel_id: this.context.channelId, action: 'image_cancel', upload_id: id }).catch(() => {});
      throw error;
    } finally {
      if (generation === this.uploadGeneration) { this.uploadId = ''; this.patch({ upload: null }); }
    }
  }
  cancelUpload() {
    ++this.uploadGeneration;
    const id = this.uploadId; this.uploadId = '';
    if (id) void this.request(this.context, { channel_id: this.context.channelId, action: 'image_cancel', upload_id: id }).catch(() => {});
    this.patch({ upload: null });
  }
  watchImage(reference: string, evicted: () => void) {
    const listeners = this.imageEvictions.get(reference) ?? new Set();
    listeners.add(evicted); this.imageEvictions.set(reference, listeners);
    return () => { listeners.delete(evicted); if (!listeners.size) this.imageEvictions.delete(reference); };
  }
  private remember(meta: RoomImageMeta, bytes: Uint8Array) {
    const old = this.imageCache.get(meta.reference);
    if (old) return old.url;
    let size = [...this.imageCache.values()].reduce((sum, image) => sum + image.bytes, 0);
    while (this.imageCache.size >= 32 || size + bytes.length > 8 * 1024 * 1024) {
      const first = this.imageCache.entries().next().value;
      if (!first) break;
      URL.revokeObjectURL(first[1].url); this.imageCache.delete(first[0]); size -= first[1].bytes;
      this.imageEvictions.get(first[0])?.forEach(listener => listener());
    }
    const url = URL.createObjectURL(new Blob([Uint8Array.from(bytes)], { type: 'image/jpeg' }));
    this.imageCache.set(meta.reference, { url, bytes: bytes.length });
    return url;
  }
  image(meta: RoomImageMeta): Promise<string> {
    if (this.disposed) return Promise.reject(canceled());
    const cached = this.imageCache.get(meta.reference);
    if (cached) { this.imageCache.delete(meta.reference); this.imageCache.set(meta.reference, cached); return Promise.resolve(cached.url); }
    const pending = this.downloads.get(meta.reference);
    if (pending) return pending;
    if (!this.imagesSupported || meta.mime !== 'image/jpeg' || meta.bytes < 1 || meta.bytes > MAX_ROOM_IMAGE_BYTES) return Promise.reject(new Error('这张图片暂时无法读取'));
    if (this.downloads.size >= 8) return Promise.reject(new Error('图片较多，点一下继续加载'));
    const task = this.downloadTail.catch(() => {}).then(async () => {
      this.valid();
      const bytes = new Uint8Array(meta.bytes);
      for (let offset = 0; offset < bytes.length;) {
        const part = await this.imageCall({ action: 'image_get', reference: meta.reference, offset });
        if (part.reference !== meta.reference || part.offset !== offset || !part.data) throw new Error('图片数据不完整，请重试');
        const raw = atob(part.data);
        if (!raw.length || raw.length > IMAGE_CHUNK_BYTES || offset + raw.length > bytes.length) throw new Error('图片数据大小不正确');
        for (let i = 0; i < raw.length; i++) bytes[offset + i] = raw.charCodeAt(i);
        offset += raw.length;
        if (!!part.eof !== (offset === bytes.length)) throw new Error('图片传输提前结束');
      }
      this.valid();
      if (bytes[0] !== 0xff || bytes[1] !== 0xd8) throw new Error('图片格式不正确');
      return this.remember(meta, bytes);
    }).finally(() => this.downloads.delete(meta.reference));
    this.downloadTail = task;
    this.downloads.set(meta.reference, task);
    return task;
  }
  dispose() {
    this.cancelUpload(); this.disposed = true;
    clearInterval(this.expiryTimer); this.listeners.clear();
    this.imageCache.forEach(image => URL.revokeObjectURL(image.url)); this.imageCache.clear(); this.imageEvictions.clear();
  }
}
