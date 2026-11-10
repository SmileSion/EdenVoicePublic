import { useEffect, useRef, useState } from 'react';
import { Expand, ImagePlus, LoaderCircle, Send, X } from 'lucide-react';
import { MAX_ROOM_IMAGE_BYTES, RoomSocialController, type RoomImageMeta } from './roomSocialController';
import './room-images.css';

export interface PreparedRoomImage { bytes: Uint8Array; width: number; height: number; url: string; originalBytes: number; }
const sizeLabel = (bytes: number) => `${Math.max(1, Math.round(bytes / 1024))} KB`;

export async function prepareRoomImage(file: File): Promise<PreparedRoomImage> {
  if (!['image/png', 'image/jpeg', 'image/webp'].includes(file.type)) throw new Error('请选择 PNG、JPEG 或 WebP 图片');
  if (!file.size || file.size > 20 * 1024 * 1024) throw new Error('原图请控制在 20 MB 以内');
  const source = URL.createObjectURL(file);
  try {
    const image = new Image(); image.src = source;
    await image.decode();
    if (!image.naturalWidth || !image.naturalHeight || image.naturalWidth * image.naturalHeight > 32_000_000) throw new Error('图片像素过大，请先裁剪后再分享');
    let scale = Math.min(1, 1600 / Math.max(image.naturalWidth, image.naturalHeight));
    for (let attempt = 0; attempt < 4; attempt++) {
      const canvas = document.createElement('canvas');
      canvas.width = Math.max(1, Math.round(image.naturalWidth * scale));
      canvas.height = Math.max(1, Math.round(image.naturalHeight * scale));
      const ctx = canvas.getContext('2d');
      if (!ctx) throw new Error('当前客户端无法处理图片');
      ctx.fillStyle = '#fff'; ctx.fillRect(0, 0, canvas.width, canvas.height);
      ctx.drawImage(image, 0, 0, canvas.width, canvas.height);
      for (const quality of [0.88, 0.76, 0.64]) {
        const blob = await new Promise<Blob>((resolve, reject) => canvas.toBlob(value => value ? resolve(value) : reject(new Error('图片压缩失败')), 'image/jpeg', quality));
        if (blob.size <= MAX_ROOM_IMAGE_BYTES) return { bytes: new Uint8Array(await blob.arrayBuffer()), width: canvas.width, height: canvas.height, url: URL.createObjectURL(blob), originalBytes: file.size };
      }
      scale *= 0.78;
    }
    throw new Error('图片内容过于复杂，请裁剪后再分享');
  } finally { URL.revokeObjectURL(source); }
}

function useImageDialog(close: () => void) {
  const dialog = useRef<HTMLElement>(null);
  const closeRef = useRef(close); closeRef.current = close;
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    dialog.current?.querySelector<HTMLButtonElement>('button')?.focus();
    const key = (event: KeyboardEvent) => {
      if (event.key === 'Escape') { event.preventDefault(); event.stopImmediatePropagation(); closeRef.current(); }
      if (event.key !== 'Tab') return;
      const nodes = Array.from(dialog.current?.querySelectorAll<HTMLElement>('button:not(:disabled),a[href],input:not(:disabled)') ?? []);
      if (!nodes.length) { event.preventDefault(); return; }
      const first = nodes[0], last = nodes[nodes.length - 1];
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
      if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
    };
    window.addEventListener('keydown', key, true);
    return () => { window.removeEventListener('keydown', key, true); if (previous?.isConnected) previous.focus(); };
  }, []);
  return dialog;
}

export function RoomImageComposer({ file, controller, roomName, close, sent }: { file: File; controller: RoomSocialController; roomName: string; close: () => void; sent: () => void }) {
  const [prepared, setPrepared] = useState<PreparedRoomImage | null>(null);
  const [error, setError] = useState('');
  const [sending, setSending] = useState(false);
  const [progress, setProgress] = useState(0);
  const active = useRef(true);
  const sendingRef = useRef(false);
  const cancel = () => { active.current = false; controller.cancelUpload(); close(); };
  const dialog = useImageDialog(cancel);
  useEffect(() => {
    active.current = true;
    let disposed = false;
    let image: PreparedRoomImage | null = null;
    void prepareRoomImage(file).then(value => {
      image = value;
      if (disposed) URL.revokeObjectURL(value.url); else setPrepared(value);
    }).catch(reason => { if (!disposed) setError(reason instanceof Error ? reason.message : '无法读取这张图片'); });
    const stop = controller.subscribe(() => { const upload = controller.snapshot().upload; if (upload && !disposed) setProgress(Math.round(upload.sent / upload.total * 100)); });
    return () => { active.current = false; disposed = true; stop(); if (image) URL.revokeObjectURL(image.url); if (sendingRef.current) controller.cancelUpload(); };
  }, [file, controller]);
  const submit = async () => {
    if (!prepared || sendingRef.current) return;
    sendingRef.current = true; setSending(true); setError(''); setProgress(0);
    try { await controller.upload(prepared.bytes, prepared.width, prepared.height); if (active.current) sent(); }
    catch (reason) { if (active.current) setError(reason instanceof Error ? reason.message : '发送失败，图片仍在这里，可以重试'); }
    finally { sendingRef.current = false; if (active.current) setSending(false); }
  };
  return <div className="room-image-backdrop" onMouseDown={event => { if (event.target === event.currentTarget) cancel(); }}><section ref={dialog} className="room-image-compose" role="dialog" aria-modal="true" aria-labelledby="room-image-title"><header><div><span className="image-eyebrow">SHARE A MOMENT</span><h3 id="room-image-title">给房间发张图</h3><p>发送到 {roomName}</p></div><button type="button" aria-label="取消发送图片" onClick={cancel}><X size={18} /></button></header><div className="room-image-stage">{prepared ? <img src={prepared.url} alt="待发送的图片预览" draggable={false} /> : <span>{error ? <ImagePlus size={28} /> : <LoaderCircle className="image-loading" size={24} />}</span>}</div>{prepared && <div className="room-image-meta"><span>{prepared.width} × {prepared.height}</span><span>{sizeLabel(prepared.bytes.length)} · 已优化大小</span></div>}{error && <p className="room-image-error" role="alert">{error}</p>}{sending && <div className="room-image-progress" role="progressbar" aria-label="图片发送进度" aria-valuemin={0} aria-valuemax={100} aria-valuenow={progress}><i style={{ width: `${progress}%` }} /></div>}<footer><span>{sending ? `正在发送 ${progress}%` : '预览确认后再发送'}</span><button type="button" className="secondary" onClick={cancel}>{sending ? '取消发送' : '取消'}</button><button type="button" className="primary" disabled={!prepared || sending} onClick={() => { void submit(); }}>{sending ? <LoaderCircle className="image-loading" size={15} /> : <Send size={15} />}{sending ? '发送中' : error && prepared ? '重新发送' : '发送图片'}</button></footer></section></div>;
}

function ImageViewer({ source, image, close }: { source: string; image: RoomImageMeta; close: () => void }) {
  const dialog = useImageDialog(close);
  return <div className="room-image-backdrop image-viewer-backdrop" onMouseDown={event => { if (event.target === event.currentTarget) close(); }}><section className="room-image-viewer" role="dialog" aria-modal="true" aria-label="查看房间图片" ref={dialog}><header><span>房间图片 <small>{image.width} × {image.height}</small></span><button type="button" aria-label="关闭图片" onClick={close}><X size={20} /></button></header><img src={source} alt="房间成员分享的完整图片" draggable={false} /><footer><a href={source} download={`edenvoice-${image.reference.slice(-12)}.jpg`}>保存图片</a><span>按 Esc 返回聊天</span></footer></section></div>;
}

export function RoomImageAttachment({ image, controller }: { image: RoomImageMeta; controller: RoomSocialController | null }) {
  const [source, setSource] = useState('');
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(false);
  const [open, setOpen] = useState(false);
  const button = useRef<HTMLButtonElement>(null);
  const generation = useRef(0);
  const load = () => {
    if (loading || source || !controller) return;
    const current = generation.current;
    setLoading(true); setError('');
    void controller.image(image).then(url => { if (current === generation.current) setSource(url); }).catch(reason => { if (current === generation.current) setError(reason instanceof Error ? reason.message : '图片加载失败，点击重试'); }).finally(() => { if (current === generation.current) setLoading(false); });
  };
  const loadRef = useRef(load); loadRef.current = load;
  useEffect(() => {
    ++generation.current; setSource(''); setError(''); setLoading(false); setOpen(false);
    const stopEviction = controller?.watchImage(image.reference, () => { setSource(''); setOpen(false); });
    const observer = new IntersectionObserver(entries => { if (entries.some(item => item.isIntersecting)) { loadRef.current(); observer.disconnect(); } });
    if (button.current) observer.observe(button.current);
    return () => { ++generation.current; observer.disconnect(); stopEviction?.(); };
  }, [controller, image.reference]);
  return <><button ref={button} type="button" className={`room-image-attachment ${source ? 'ready' : ''}`} aria-label={source ? '查看完整图片' : '加载房间图片'} style={{ aspectRatio: `${Math.max(1, image.width)} / ${Math.max(1, image.height)}` }} onClick={() => { if (source) setOpen(true); else load(); }}>{source ? <><img src={source} alt="房间截图" loading="lazy" draggable={false} /><span className="image-expand"><Expand size={14} />查看图片</span></> : <span className="image-placeholder">{loading ? <LoaderCircle className="image-loading" size={22} /> : <ImagePlus size={22} />}<small>{error || (loading ? '正在加载图片…' : controller?.imagesSupported ? '点击查看图片' : '当前连接无法读取图片')}</small></span>}</button>{open && source && <ImageViewer source={source} image={image} close={() => setOpen(false)} />}</>;
}
