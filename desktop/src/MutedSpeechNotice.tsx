import { useEffect, useRef, useState } from 'react';
import { Mic, MicOff, X } from 'lucide-react';
import { dispatch } from './bridge';
import type { CoreSnapshot } from './types';
import { hasVoiceChannel } from './room-presentation';
import './muted-speech.css';

/** Only presents the native speech detector's event; never reads microphone audio. */
export function MutedSpeechNotice({ snapshot }: { snapshot: CoreSnapshot }) {
  const self = snapshot.users.find(user => user.sessionId === snapshot.selfSessionId);
  const room = JSON.stringify([snapshot.serverAddress, snapshot.selfSessionId, self?.channelId]);
  const notice = snapshot.mutedSpeechNotice;
  const allowed = snapshot.connection === 'connected' && hasVoiceChannel(self?.channelId)
    && snapshot.muted && !snapshot.deafened && !snapshot.selfAway && !self?.adminMuted;
  const consumed = useRef('');
  const [visible, setVisible] = useState<{ room: string; until: number } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  useEffect(() => {
    const key = `${snapshot.serverAddress}:${snapshot.selfSessionId}:${notice?.sequence ?? ''}`;
    const remaining = Math.min(8000, 8000 - (Date.now() - (notice?.atUnixMs ?? 0)));
    if (!allowed || !notice || remaining <= 0) { setVisible(null); return; }
    if (key === consumed.current) return;
    consumed.current = key;
    setVisible({ room, until: Date.now() + remaining }); setError('');
  }, [room, allowed, notice?.sequence, notice?.atUnixMs]);
  useEffect(() => {
    if (!visible) return;
    const timeout = window.setTimeout(() => setVisible(null), Math.max(0, visible.until - Date.now()));
    return () => window.clearTimeout(timeout);
  }, [visible]);
  if (!allowed || visible?.room !== room) return null;
  const unmute = async () => {
    if (busy) return;
    setBusy(true); setError('');
    try { await dispatch({ type: 'set_muted', muted: false }); setVisible(null); }
    catch { setError('开麦未成功，请重试'); }
    finally { setBusy(false); }
  };
  return <aside className="muted-speech-notice" role="status" aria-live="polite">
    <span className="muted-speech-mark"><MicOff size={18}/></span>
    <div><strong>你可能正在说话</strong><small>{error || '麦克风仍然关闭，房间里的朋友听不到'}</small></div>
    <button type="button" className="muted-speech-unmute" disabled={busy} onClick={() => void unmute()}><Mic size={13}/>{busy ? '正在开麦' : '打开麦克风'}</button>
    <button type="button" className="muted-speech-close" aria-label="关闭静音说话提醒" onClick={() => setVisible(null)}><X size={15}/></button>
  </aside>;
}
