import {VoiceIncident} from "./VoiceIncident";
import {Advice,DeviceNotice,NotificationControls,QuickJump} from './ExperienceControls';
import {roomKey,roomPreferences,updateRoom,preferences,newerMessage,compareMessages} from './experienceState';
import {useRoomDraft} from './useRoomDraft';
import {useUnreadRooms} from './useUnreadRooms';
import {useChatHistory} from './useChatHistory';
import { RoomImageAttachment, RoomImageComposer } from './RoomImage';
import { useRoomSocial } from './useRoomSocial';
import { RoomMemberPresence, type RoomSocialView } from './RoomPresence';
import { MutedSpeechNotice } from './MutedSpeechNotice';
import type { RoomSocialController } from './roomSocialController';
import { ScreenShare } from "./ScreenShare";
import { ArchiveJournal } from "./ArchiveTimeline";
import { ArchiveRecap } from "./ArchiveRecap";
import {ServerInvitation} from "./ServerInvitation";
import {createInvitation,parseInvitation} from "./invitations";
import { AnnouncementNotice } from "./AnnouncementNotice";
import { UpdateNotice } from "./UpdateNotice";
import { PasswordRoom } from "./PasswordRoom";
import { FocusTimer } from "./FocusTimer";
import { gameActivityLabel, hasVoiceChannel } from "./room-presentation";
import { CloseBehaviorDialog, CloseBehaviorSettings } from "./CloseBehavior";
import {
  Activity,
  ArrowRightLeft,
  Check,
  ChevronDown,
  ChevronRight,
  CircleAlert,
  CircleUserRound,
  Copy,
  Download,
  Crown,
  HeadphoneOff,
  Headphones,
  Info,
  ImagePlus,
  LogOut,
  LockKeyhole,
  Maximize2,
  MessageCircle,
  Mic,
  MicOff,
  Minus,
  MoreHorizontal,
  Palette,
  Plus,
  PictureInPicture2,
  Radio,
  RefreshCw,
  Search,
  Send,
  Server,
  Settings,
  Shield,
  ShieldCheck,
  Signal,
  Smile,
  Shrink,
  Upload,
  Trash2,
  Users,
  Volume2,
  VolumeX,
  Wifi,
  X,
} from "lucide-react";
import { type CSSProperties, type FormEvent, type KeyboardEvent as ReactKeyboardEvent, type MouseEvent, useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { dispatch as dispatchCore, exportUserData, importUserData, listenForRoomArchive, listenForSnapshots, restartClient, saveDiagnosticReport, startWindowDragging, resizeMemberPanel, windowAction } from "./bridge";
import type { ArchiveEvent, ArchiveParticipantProfile, BanRecord, Channel, CoreCommand, CoreSnapshot, RoomArchive, SavedServer, ShortcutState, User } from "./types";
import { PlayerSettings } from "./PlayerSettings";
import { Avatar, AvatarCacheContext } from "./Avatar";
import { RoomExperience } from "./WatchParty";
import { playUiSound } from "./uiSounds";
import { roomNotificationSounds, roomNotificationState, type RoomNotificationState } from "./roomNotifications";
import { chatTimestamp, isOwnChatMessage } from "./chatPresentation";
import { useChatScroll } from "./useChatScroll";
import appIcon from "../brand/app-icon-dark.svg";
import lightAppIcon from "../brand/app-icon-light.svg";

type SettingsPage = "player" | "profile" | "audio" | "diagnostics" | "servers" | "appearance" | "shortcuts" | "about";

const MINI_WINDOW = new URLSearchParams(window.location.search).has("mini");
if (MINI_WINDOW) document.documentElement.classList.add("mini-window");

async function dispatch(command: CoreCommand): Promise<boolean> {
  try {
    await dispatchCore(command);
    return true;
  } catch (error) {
    window.dispatchEvent(new CustomEvent("eden-command-error", { detail: String(error) }));
    return false;
  }
}

type FlatSelectOption = { value: string; label: string; help?: string };

const NOISE_MODE_OPTIONS: FlatSelectOption[] = [
  { value: "off", label: "关闭降噪", help: "保留麦克风原声，不进行环境降噪。" },
  { value: "standard", label: "日常降噪", help: "WebRTC APM：适合日常通话，抑制键鼠、碰撞和回声，并自动稳定人声音量。" },
  { value: "rnnoise", label: "轻柔降噪", help: "RNNoise：低延迟、低资源占用，适合风扇、空调和底噪，尽量保留自然人声。" },
  { value: "voice_isolation", label: "人声隔离", help: "DeepFilterNet：针对复杂环境声和近距离杂音，可选自然或强力，约增加 70 ms 处理延迟。" },
];

function noiseModeCategory(value: string) {
  return value === "deepfilternet" || value === "deepfilternet_strong" ? "voice_isolation" : value;
}

function noiseModeHelp(value: string) {
  return NOISE_MODE_OPTIONS.find((option) => option.value === noiseModeCategory(value))?.help ?? "";
}

function selectNoiseMode(category: string, current: string) {
  if (category !== "voice_isolation") return category;
  return noiseModeCategory(current) === "voice_isolation" ? current : "deepfilternet";
}

function InfoHint({ text }: { text: string }) {
  return <span className="info-hint" tabIndex={0} aria-label={text}><CircleAlert size={13} aria-hidden="true" /><span className="info-hint-popover" role="tooltip">{text}</span></span>;
}

const ROOM_AUDIO_QUALITY_BITRATES = [16, 24, 32, 40, 48, 56, 64, 72, 80, 96];

function normalizeRoomAudioQuality(value: string) {
  const legacyLevels: Record<string, string> = {
    fixed_32k: "quality_3",
    fixed_48k: "quality_5",
    fixed_64k: "quality_7",
    fixed_96k: "quality_10",
  };
  const normalized = legacyLevels[value] ?? value;
  const level = Number(normalized.replace("quality_", ""));
  return normalized === "auto" || (Number.isInteger(level) && level >= 1 && level <= 10) ? normalized : "auto";
}

function roomAudioQualityBadge(value: string) {
  const normalized = normalizeRoomAudioQuality(value);
  return normalized === "auto" ? "" : `Q${normalized.replace("quality_", "")}`;
}

function RoomAudioQualityPicker({ value, onChange }: { value: string; onChange: (value: string) => void }) {
  const normalized = normalizeRoomAudioQuality(value);
  const automatic = normalized === "auto";
  const level = automatic ? 5 : Number(normalized.replace("quality_", ""));
  const selectLevel = (next: number) => onChange(`quality_${next}`);
  return <div className={`room-quality-picker ${automatic ? "automatic" : ""}`}>
    <header><span><strong>{automatic ? "自动适应网络" : `音质等级 ${level}`}</strong><small>{automatic ? "根据网络状况动态调整" : `${ROOM_AUDIO_QUALITY_BITRATES[level - 1]} kbps · 房间统一音质`}</small></span><button type="button" className={automatic ? "selected" : ""} onClick={() => onChange("auto")}><Check size={11} />自动</button></header>
    <input aria-label="房间音质等级" type="range" min="1" max="10" step="1" value={level} onChange={(event) => selectLevel(Number(event.target.value))} />
    <div className="room-quality-ticks" aria-label="房间音质等级 1 到 10">{ROOM_AUDIO_QUALITY_BITRATES.map((bitrate, index) => { const next = index + 1; return <button type="button" className={!automatic && level === next ? "selected" : ""} aria-label={`音质等级 ${next}，${bitrate} kbps`} key={next} onClick={() => selectLevel(next)}>{next}</button>; })}</div>
  </div>;
}

function BrandIcon({ className }: { className: string }) {
  return <span className={`${className} brand-icon`} aria-hidden="true"><img draggable={false} onDragStart={event=>event.preventDefault()} className="brand-icon-dark" src={appIcon} alt="" /><img draggable={false} onDragStart={event=>event.preventDefault()} className="brand-icon-light" src={lightAppIcon} alt="" /></span>;
}

function FlatSelect({ value, options, onChange, label, disabled = false }: { value: string; options: FlatSelectOption[]; onChange: (value: string) => void; label: string; disabled?: boolean }) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const selected = options.find((option) => option.value === value) ?? options[0];
  useEffect(() => {
    if (!open) return;
    const close = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node)) setOpen(false); };
    const escape = (event: globalThis.KeyboardEvent) => { if (event.key === "Escape") setOpen(false); };
    window.addEventListener("pointerdown", close);
    window.addEventListener("keydown", escape);
    return () => { window.removeEventListener("pointerdown", close); window.removeEventListener("keydown", escape); };
  }, [open]);
  return <div className={`flat-select ${open ? "open" : ""} ${disabled ? "disabled" : ""}`} ref={root}>
    <button type="button" className="flat-select-trigger" aria-label={label} aria-haspopup="listbox" aria-expanded={open} disabled={disabled} onClick={() => setOpen((current) => !current)}><span>{selected?.label}</span><ChevronDown size={14} /></button>
    {open && <div className="flat-select-options" role="listbox" aria-label={label}>{options.map((option) => <button type="button" role="option" aria-label={option.label} aria-selected={option.value === value} key={option.value} onClick={() => { onChange(option.value); setOpen(false); }}><span>{option.label}</span>{option.value === value && <Check size={13} />}</button>)}</div>}
  </div>;
}

function audioDeviceOptions(defaultLabel: string, devices: string[]) {
  return [{ value: "", label: defaultLabel }, ...devices.filter((device) => device && device !== defaultLabel).map((device) => ({ value: device, label: device }))];
}

function normalizeAudioDevice(value: string, defaultLabel: string) {
  return value === defaultLabel ? "" : value;
}

function normalizeInputMode(value: string) {
  return value === "push_to_talk" || value === "pushtotalk" ? "push_to_talk" : "voice_activity";
}

function metric(value: number | null, suffix = "") {
  return value == null ? "--" : `${Number.isInteger(value) ? value : value.toFixed(1)}${suffix}`;
}

async function diagnosticHash(value: string) {
  if (!value) return "";
  if (!globalThis.crypto?.subtle) return `redacted-${value.length}`;
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(value));
  return Array.from(new Uint8Array(digest).slice(0, 8), (byte) => byte.toString(16).padStart(2, "0")).join("");
}

async function exportDiagnostics(snapshot: CoreSnapshot) {
  const [serverAddressHash, clientIdHash, inputDeviceHash, outputDeviceHash] = await Promise.all([
    diagnosticHash(snapshot.serverAddress),
    diagnosticHash(snapshot.clientId),
    diagnosticHash(snapshot.audio.activeInput),
    diagnosticHash(snapshot.audio.activeOutput),
  ]);
  const report = {
    schema: "edenvoice.voice-diagnostics.v1",
    appVersion: "0.8.5",
    generatedAt: new Date().toISOString(),
    platform: navigator.platform || "unknown",
    connection: snapshot.connection,
    serverAddressHash,
    clientIdHash,
    diagnostics: snapshot.diagnostics,
    audio: {
      state: snapshot.audio.state,
      inputDeviceHash,
      outputDeviceHash,
      inputSampleRate: snapshot.audio.inputSampleRate,
      outputSampleRate: snapshot.audio.outputSampleRate,
      noiseSuppression: snapshot.audio.noiseSuppression,
      echoCancellation: snapshot.audio.echoCancellation,
      automaticGain: snapshot.audio.automaticGain,
      deviceErrorPresent: Boolean(snapshot.audio.deviceError),
      dspErrorPresent: Boolean(snapshot.audio.dspError),
    },
  };
  return saveDiagnosticReport(JSON.stringify(report));
}

function DiagnosticsSettings({ snapshot, onAction }: { snapshot: CoreSnapshot; onAction?: (action:'audio'|'network'|'diagnostics')=>void }) {
  const [exportStatus, setExportStatus] = useState("");
  const diagnostics = snapshot.diagnostics;
  const audio = snapshot.audio;
  const cards = [
    ["媒体 RTT", metric(diagnostics.rttMs, " ms")],
    ["网络 Jitter", metric(diagnostics.jitterMs, " ms")],
    ["有效丢包", metric(diagnostics.effectiveAudioLossPercent, "%")],
    ["实际链路", diagnostics.transport || "OFFLINE"],
    ["Opus 码率", metric(Math.round(diagnostics.opusBitrateBps / 1000), " kbps")],
    ["缓冲延迟", metric(diagnostics.jitterBufferDelayMs, " ms")],
  ];
  return <div className="diagnostics-settings"><VoiceIncident/>
    <Advice snapshot={snapshot} onAction={onAction}/>
    <div className="diagnostics-heading"><div><h3>语音诊断</h3><p className="section-description">实时显示当前语音链路状态，计数从本次音频会话开始累计。</p></div><button type="button" className="diagnostics-export" onClick={async () => { try { const path = await exportDiagnostics(snapshot); setExportStatus(`已导出：${path}`); } catch (error) { setExportStatus(`导出失败：${String(error)}`); } }}><Download size={14} />导出脱敏报告</button></div>
    {exportStatus && <p className="diagnostics-export-status" role="status">{exportStatus}</p>}
    <div className="diagnostics-grid">{cards.map(([label, value]) => <div className="diagnostic-card" key={label}><span>{label}</span><strong>{value}</strong></div>)}</div>
    <h3>丢包恢复与播放</h3>
    <div className="diagnostics-table">
      <div><span>网络质量</span><strong>{diagnostics.networkQuality || "--"}</strong></div>
      <div><span>FEC 恢复</span><strong>{diagnostics.fecRecoveredPackets}</strong></div>
      <div><span>PLC 补偿</span><strong>{diagnostics.plcFrames}</strong></div>
      <div><span>Jitter 缓冲深度</span><strong>{diagnostics.jitterDepthFrames} 帧</strong></div>
      <div><span>Jitter 欠载</span><strong>{diagnostics.jitterBufferUnderruns}</strong></div>
      <div><span>设备播放欠载</span><strong>{diagnostics.playbackUnderruns}</strong></div>
      <div><span>采集队列覆盖</span><strong>{diagnostics.captureOverruns}</strong></div>
      {diagnostics.timing && <>
        <div><span>采集帧龄</span><strong>{diagnostics.timing.captureAgeMs.toFixed(1)} ms</strong></div>
        <div><span>播放队列 / 实际抖动缓冲</span><strong>{diagnostics.timing.playbackQueueMs.toFixed(0)} / {diagnostics.timing.actualJitterMs.toFixed(1)} ms</strong></div>
        <div><span>音频处理 P95 / P99</span><strong>{diagnostics.timing.workerP95Ms} / {diagnostics.timing.workerP99Ms} ms</strong></div>
        <div><span>播放调度迟到 P99</span><strong>{diagnostics.timing.playoutLatenessP99Ms} ms</strong></div>
        <div><span>发送帧携带 FEC</span><strong>{diagnostics.timing.sentFecPercent.toFixed(1)}%</strong></div>
        <div><span>采集处理 P95</span><strong>{diagnostics.timing.captureProcessingP95Ms} ms</strong></div>
        <div><span>采集过期丢弃 / 降噪补静音</span><strong>{diagnostics.timing.captureStaleDrops} / {diagnostics.timing.dspSilenceFrames}</strong></div>
        <div><span>AEC 延迟估计 / 参考丢帧</span><strong>{diagnostics.timing.aecDelayMs.toFixed(1)} ms / {diagnostics.timing.renderReferenceDrops}</strong></div>
      </>}
      <div><span>收包 / 丢包 / 乱序</span><strong>{diagnostics.packetsReceived} / {diagnostics.packetsLost} / {diagnostics.packetsReordered}</strong></div>
    </div>
    <h3>音频设备</h3>
    <div className="diagnostics-devices"><div><span>输入</span><strong>{audio.activeInput || "尚未打开"}</strong><small>{audio.inputSampleRate ? `${audio.inputSampleRate / 1000} kHz` : "--"}</small></div><div><span>输出</span><strong>{audio.activeOutput || "尚未打开"}</strong><small>{audio.outputSampleRate ? `${audio.outputSampleRate / 1000} kHz` : "--"}</small></div></div>
    {[audio.inputSampleRate, audio.outputSampleRate].some((rate) => rate != null && rate > 0 && rate < 32000) && <p className="audio-sample-rate-note">当前设备采样率较低，可能限制声音清晰度。若使用蓝牙耳机，可改用独立麦克风或有线耳机对比；内部升采样无法还原设备没有采集到的高频声音。</p>}
    <p className="diagnostics-privacy">导出报告不会包含昵称、聊天内容、服务器明文地址、Client ID 或设备名称；这些标识仅保留不可逆短哈希以便关联同一次问题。</p>
  </div>;
}

async function copyText(value: string) {
  try {
    await navigator.clipboard.writeText(value);
    return true;
  } catch {
    const field = document.createElement("textarea");
    field.value = value;
    field.style.position = "fixed";
    field.style.opacity = "0";
    document.body.appendChild(field);
    field.select();
    const copied = document.execCommand("copy");
    field.remove();
    return copied;
  }
}

function contextMenuPoint(event: MouseEvent, width: number) {
  return {
    x: Math.max(8, Math.min(event.clientX, window.innerWidth - width - 8)),
    y: Math.max(46, Math.min(event.clientY, window.innerHeight - 150)),
  };
}


function shortcutFromEvent(event: Pick<KeyboardEvent, "altKey" | "code" | "ctrlKey" | "metaKey" | "shiftKey">) {
  if (["ControlLeft", "ControlRight", "AltLeft", "AltRight", "ShiftLeft", "ShiftRight", "MetaLeft", "MetaRight"].includes(event.code)) return "";
  const parts: string[] = [];
  if (event.ctrlKey) parts.push("Control");
  if (event.altKey) parts.push("Alt");
  if (event.shiftKey) parts.push("Shift");
  if (event.metaKey) parts.push("Super");
  parts.push(event.code);
  return parts.join("+");
}

function shortcutLabel(binding: string) {
  if (!binding) return "点击绑定";
  return binding.split("+").map((part) => part === "Control" ? "Ctrl" : part === "Super" ? "Cmd" : part.startsWith("Key") ? part.slice(3) : part.startsWith("Digit") ? part.slice(5) : part === "Backquote" ? "`" : part).join(" + ");
}

function ShortcutCapture({ value, onChange }: { value: string; onChange: (value: string) => void }) {
  const [listening, setListening] = useState(false);
  const modifierStarted = useRef<number | null>(null);
  const timer = useRef<number | null>(null);
  const stop = () => {
    setListening(false);
    modifierStarted.current = null;
    if (timer.current != null) window.clearTimeout(timer.current);
    timer.current = null;
  };
  const record = (event: ReactKeyboardEvent<HTMLButtonElement>) => {
    event.preventDefault();
    event.stopPropagation();
    const isModifier = ["ControlLeft", "ControlRight", "AltLeft", "AltRight", "ShiftLeft", "ShiftRight", "MetaLeft", "MetaRight"].includes(event.code);
    if (isModifier) {
      if (modifierStarted.current == null) {
        modifierStarted.current = performance.now();
        timer.current = window.setTimeout(stop, 500);
      }
      return;
    }
    if (modifierStarted.current != null && performance.now() - modifierStarted.current > 500) return stop();
    const binding = shortcutFromEvent(event.nativeEvent);
    if (binding) onChange(binding);
    stop();
  };
  return <button className={listening ? "recording" : ""} onClick={(event) => { event.stopPropagation(); setListening(true); }} onKeyDown={record} onBlur={stop} onDoubleClick={() => { stop(); onChange(""); }} title="双击清除绑定">{listening ? "请按下按键…" : shortcutLabel(value)}</button>;
}


function AvatarEditor({ source, close }: { source: string; close: () => void }) {
  const [zoom, setZoom] = useState(1);
  const [offset, setOffset] = useState({ x: 0, y: 0 });
  const [drag, setDrag] = useState<{ x: number; y: number; ox: number; oy: number } | null>(null);
  const [error, setError] = useState("");
  const save = async () => {
    const image = new Image();
    image.src = source;
    await image.decode();
    const canvas = document.createElement("canvas");
    canvas.width = 128;
    canvas.height = 128;
    const context = canvas.getContext("2d");
    if (!context) return setError("浏览器无法创建头像画布");
    context.fillStyle = "#202126";
    context.fillRect(0, 0, 128, 128);
    const scale = Math.max(128 / image.naturalWidth, 128 / image.naturalHeight) * zoom;
    const width = image.naturalWidth * scale;
    const height = image.naturalHeight * scale;
    context.drawImage(image, (128 - width) / 2 + offset.x * (128 / 180), (128 - height) / 2 + offset.y * (128 / 180), width, height);
    let data = "";
    for (let quality = 0.86; quality >= 0.38; quality -= 0.06) {
      data = canvas.toDataURL("image/jpeg", quality);
      if (atob(data.split(",")[1]).length <= 12 * 1024) break;
    }
    if (!data || atob(data.split(",")[1]).length > 12 * 1024) return setError("图片内容过于复杂，无法压缩到头像大小限制");
    await dispatch({ type: "set_avatar", avatar: data });
    close();
  };
  return <div className="crop-backdrop"><section className="crop-editor">
    <header><div><h3>调整头像</h3><p>拖动图片选择区域，使用滑杆缩放</p></div><button onClick={close}><X size={18} /></button></header>
    <div className="crop-stage" onPointerDown={(event) => { event.currentTarget.setPointerCapture(event.pointerId); setDrag({ x: event.clientX, y: event.clientY, ox: offset.x, oy: offset.y }); }} onPointerMove={(event) => drag && setOffset({ x: drag.ox + event.clientX - drag.x, y: drag.oy + event.clientY - drag.y })} onPointerUp={() => setDrag(null)}>
      <img draggable={false} onDragStart={event=>event.preventDefault()} src={source} alt="待裁剪头像" style={{ transform: `translate(${offset.x}px, ${offset.y}px) scale(${zoom})` }} />
      <span className="crop-mask" />
    </div>
    <label className="crop-zoom"><span>缩放</span><input type="range" min="1" max="4" step="0.05" value={zoom} onChange={(event) => setZoom(Number(event.target.value))} /><em>{zoom.toFixed(1)}×</em></label>
    {error && <p className="crop-error">{error}</p>}
    <footer><button onClick={close}>取消</button><button className="primary" onClick={save}>使用此头像</button></footer>
  </section></div>;
}

function connectionDisplay(snapshot: CoreSnapshot) {
  const inVoice = snapshot.connection === "connected" && hasVoiceChannel(snapshot.users.find(u => u.sessionId === snapshot.selfSessionId)?.channelId);
  const audioFailed = snapshot.audio.state.startsWith("error") || snapshot.audio.state === "stopped";
  return { inVoice, unavailable: (inVoice && audioFailed) || snapshot.connection === "reconnecting", label: inVoice ? audioFailed ? "语音异常" : snapshot.audio.state === "ready" ? snapshot.transport || "语音已连接" : "语音准备中" : snapshot.connection === "connected" ? "在线" : snapshot.connection === "reconnecting" ? "重连中" : "未连接", latency: inVoice ? snapshot.audio.state === "ready" ? snapshot.latencyMs : null : snapshot.controlLatencyMs };
}

function TitleBar({ snapshot, onCompact }: { snapshot: CoreSnapshot; onCompact: () => void }) {
  const network = connectionDisplay(snapshot);
  const quality = snapshot.networkQuality === "good" ? "网络良好" : snapshot.networkQuality || "网络检测中";
  const canCompact = snapshot.connection === "connected" || snapshot.connection === "reconnecting";
  return (
    <header className="titlebar" data-tauri-drag-region>
      <div className="brand" data-tauri-drag-region><BrandIcon className="brand-mark" /><span>EdenVoice</span></div>
      <div className={`connection-chip ${network.unavailable ? "unavailable" : ""}`} data-tauri-drag-region>
        <Signal size={13} /><strong>{network.label}</strong><span>{network.latency == null ? "--" : `${network.latency} ms`}</span><i title={network.unavailable ? "连接或语音暂不可用" : quality} />
      </div>
      <div className="title-actions"><FocusTimer/>
        <div className="title-overlay-control">
          <button className={snapshot.miniOverlayEnabled ? "active" : ""} aria-label={snapshot.miniOverlayEnabled ? "关闭迷你悬浮窗" : "开启迷你悬浮窗"} title={snapshot.miniOverlayEnabled ? "关闭迷你悬浮窗" : "开启迷你悬浮窗"} onClick={() => dispatch({ type: "set_mini_overlay", enabled: !snapshot.miniOverlayEnabled, mode: snapshot.miniOverlayMode })}><PictureInPicture2 size={15} /><i /></button>
          <section className="title-overlay-menu"><header><div><strong>迷你悬浮窗</strong><span>{snapshot.miniOverlayEnabled ? "已开启" : "已关闭"}</span></div><button className={`title-overlay-switch ${snapshot.miniOverlayEnabled ? "on" : ""}`} aria-label={snapshot.miniOverlayEnabled ? "关闭迷你悬浮窗" : "开启迷你悬浮窗"} onClick={() => dispatch({ type: "set_mini_overlay", enabled: !snapshot.miniOverlayEnabled, mode: snapshot.miniOverlayMode })}><i /></button></header><small>显示模式</small><div className="title-overlay-modes"><button className={snapshot.miniOverlayMode === "avatars" ? "selected" : ""} onClick={() => dispatch({ type: "set_mini_overlay", enabled: snapshot.miniOverlayEnabled, mode: "avatars" })}><CircleUserRound size={14} /><span><strong>头像模式</strong><small>更小、更少遮挡</small></span>{snapshot.miniOverlayMode === "avatars" && <Check size={12} />}</button><button className={snapshot.miniOverlayMode === "list" ? "selected" : ""} onClick={() => dispatch({ type: "set_mini_overlay", enabled: snapshot.miniOverlayEnabled, mode: "list" })}><Users size={14} /><span><strong>列表模式</strong><small>显示成员名称</small></span>{snapshot.miniOverlayMode === "list" && <Check size={12} />}</button></div></section>
        </div>
        {canCompact && <button aria-label="切换到紧凑模式" title="切换到紧凑模式" onClick={onCompact}><Shrink size={15} /></button>}
        <button aria-label="最小化窗口" title="最小化窗口" onClick={() => windowAction("minimize")}><Minus size={15} /></button>
        <button aria-label="最大化" onClick={() => windowAction("maximize")}><span className="maximize-icon" /></button>
        <button aria-label="关闭窗口" title="关闭窗口" onClick={() => windowAction("close")}><X size={15} /></button>
      </div>
    </header>
  );
}

function SavedServerSettings({ snapshot }: { snapshot: CoreSnapshot }) {
  const [profiles, setProfiles] = useState(snapshot.savedServers);
  const [draft, setDraft] = useState<SavedServer>({ displayName: "", serverName: "", address: "", nickname: snapshot.nickname });
  const [adding, setAdding] = useState(false);
  const serverKey = JSON.stringify(snapshot.savedServers);
  useEffect(() => setProfiles(snapshot.savedServers), [serverKey]);
  const updateProfile = (index: number, key: "displayName" | "address" | "nickname", value: string) => setProfiles((current) => current.map((profile, profileIndex) => profileIndex === index ? { ...profile, [key]: value } : profile));
  const add = () => {
    if (!draft.address.trim()) return;
    dispatch({ type: "add_saved_server", profile: { ...draft, displayName: draft.displayName.trim(), address: draft.address.trim(), nickname: draft.nickname.trim() } });
    setDraft({ displayName: "", serverName: "", address: "", nickname: snapshot.nickname });
    setAdding(false);
  };
  return <div className="saved-server-settings">
    <div className="server-settings-heading"><div><h3>已保存服务器</h3><p>修改服务器备注、连接地址和进入服务器时使用的昵称。</p></div><button type="button" className="add-server-button" onClick={() => setAdding((value) => !value)}><Plus size={14} />添加服务器</button></div>
    {adding && <article className="saved-server-card new"><header><strong>新服务器</strong><span>手动添加后可在连接页和服务器菜单快速切换</span></header><div className="saved-server-fields"><label>服务器名称<input autoFocus value={draft.displayName} onChange={(event) => setDraft({ ...draft, displayName: event.target.value })} placeholder="例如：游戏开黑" /></label><label>服务器地址<input value={draft.address} onChange={(event) => setDraft({ ...draft, address: event.target.value })} placeholder="example.com:7001" /></label><label>此服务器昵称<input value={draft.nickname} maxLength={64} onChange={(event) => setDraft({ ...draft, nickname: event.target.value })} placeholder="进入服务器时使用" /></label></div><footer><button type="button" onClick={() => setAdding(false)}>取消</button><button type="button" className="primary" disabled={!draft.address.trim()} onClick={add}>添加</button></footer></article>}
    {profiles.length === 0 && !adding ? <div className="empty-profile">还没有保存的服务器，可以在这里添加或在连接时勾选“记住此服务器”</div> : <div className="saved-server-cards">{profiles.map((profile, index) => <article className="saved-server-card" key={`${profile.address}-${index}`}><header><div><strong>{profile.displayName || profile.serverName || "未命名服务器"}</strong><span>{profile.serverName && profile.serverName !== profile.displayName ? `服务器名称：${profile.serverName}` : `配置 ${index + 1}`}</span></div><button type="button" className="delete-server" aria-label="删除服务器" title="删除服务器" onClick={() => { setProfiles((current) => current.filter((_, profileIndex) => profileIndex !== index)); dispatch({ type: "delete_saved_server", index }); }}><Trash2 size={14} /></button></header><div className="saved-server-fields"><label>服务器名称<input value={profile.displayName} onChange={(event) => updateProfile(index, "displayName", event.target.value)} placeholder={profile.serverName || "自定义名称"} /></label><label>服务器地址<input value={profile.address} onChange={(event) => updateProfile(index, "address", event.target.value)} /></label><label>此服务器昵称<input value={profile.nickname} maxLength={64} onChange={(event) => updateProfile(index, "nickname", event.target.value)} placeholder="使用个人资料名称" /></label></div><footer><button type="button" className="primary" disabled={!profile.address.trim()} onClick={() => dispatch({ type: "update_saved_server", index, profile: { ...profile, displayName: profile.displayName.trim(), address: profile.address.trim(), nickname: profile.nickname.trim() } })}>保存此配置</button></footer></article>)}</div>}
  </div>;
}

function IdentityManager({ snapshot, close }: { snapshot: CoreSnapshot; close: () => void }) {
  return <div className="modal-backdrop compact-backdrop" onMouseDown={close}><section className="identity-manager server-manager-modal" onMouseDown={(event) => event.stopPropagation()}><header><div><h2>服务器管理</h2><p>管理本机保存的服务器和每台服务器使用的昵称。</p></div><button onClick={close}><X size={18} /></button></header><div className="identity-content"><SavedServerSettings snapshot={snapshot} /></div><footer><button className="primary" onClick={close}>完成</button></footer></section></div>;
}

function ConnectionScreen({ snapshot }: { snapshot: CoreSnapshot }) {
  const [serverPassword,setServerPassword]=useState("");
  const [inviteError,setInviteError]=useState("");
  useEffect(()=>{setAddress(snapshot.serverAddress);setServerPassword("");setInviteError("");},[snapshot.serverAddress]);
  const [address, setAddress] = useState(snapshot.serverAddress);
  const [nickname, setNickname] = useState(snapshot.nickname);
  const [remember, setRemember] = useState(true);
  const [autoConnect, setAutoConnect] = useState(snapshot.autoConnectLastServer);
  const [identityOpen, setIdentityOpen] = useState(false);
  const [submitAttempted, setSubmitAttempted] = useState(false);
  const [cancelingAutoConnect, setCancelingAutoConnect] = useState(false);
  const connecting = snapshot.connection === "connecting" || snapshot.connection === "reconnecting";
  const automaticConnecting = connecting && (snapshot.autoConnecting || cancelingAutoConnect);
  const addressMissing = submitAttempted && !address.trim();
  const nicknameMissing = submitAttempted && !nickname.trim();
  useEffect(() => {
    if (!connecting) setCancelingAutoConnect(false);
  }, [connecting]);
  const submit = (event: FormEvent) => {
    event.preventDefault();
    setSubmitAttempted(true);
    if (inviteError) return;
    if (!address.trim() || !nickname.trim()) return;
    dispatch({ type: "connect", serverPassword, address: address.trim(), nickname: nickname.trim(), remember: remember || autoConnect, autoConnect });
  };
  if (automaticConnecting) {
    const savedTarget = snapshot.savedServers.find((profile) => profile.address === snapshot.serverAddress);
    const targetName = savedTarget?.displayName || savedTarget?.serverName || "最近连接的服务器";
    const retrying = snapshot.connection === "reconnecting";
    return (
      <main className="connect-page auto-connect-page">
        <div className="connect-orbit"><span /><span /><span /></div>
        <section className="connect-card auto-connect-card" aria-live="polite">
          <BrandIcon className="connect-logo" />
          <div className="auto-connect-status"><span className="spinner" /></div>
          <h1>{cancelingAutoConnect ? "正在取消连接" : retrying ? "正在重新连接" : "正在连接服务器"}</h1>
          <p>{cancelingAutoConnect ? "正在安全地结束本次连接，请稍候…" : retrying ? "服务器暂时没有响应，正在尝试恢复连接…" : "正在恢复上次使用的语音空间…"}</p>
          <div className="auto-connect-target"><Server size={18} /><div><strong>{targetName}</strong><span>{snapshot.serverAddress}</span></div></div>
          <button type="button" className="auto-connect-cancel" disabled={cancelingAutoConnect} onClick={() => { setCancelingAutoConnect(true); dispatch({ type: "disconnect" }); }}>{cancelingAutoConnect ? "正在取消…" : "取消自动连接"}</button>
          <small>取消后返回连接页，不会关闭下次启动时的自动连接</small>
        </section>
      </main>
    );
  }
  return (
    <main className="connect-page">
      <div className="connect-orbit"><span /><span /><span /></div>
      <form className="connect-card" onSubmit={submit}>
        <BrandIcon className="connect-logo" />
        <h1>回到你的语音空间</h1>
        <p>轻量、稳定，专注每一次交流。</p>
        {snapshot.savedServers.length > 0 && <label>已保存服务器<FlatSelect label="已保存服务器" value={String(snapshot.savedServers.findIndex((profile) => profile.address === address))} options={[{ value: "-1", label: "手动输入" }, ...snapshot.savedServers.map((profile, index) => ({ value: String(index), label: profile.displayName || profile.serverName || profile.address }))]} onChange={(value) => { const profile = snapshot.savedServers[Number(value)]; if (profile) { setAddress(profile.address); setServerPassword(""); setInviteError(""); setNickname(profile.nickname || snapshot.nickname); } }} /></label>}
        <label>服务器地址<input value={address} onChange={(e) => {const value=e.target.value;setServerPassword("");setInviteError("");if(value.trim().startsWith("edenvoice:")){try{setAddress(parseInvitation(value.trim()));}catch{setAddress(value);setInviteError("邀请链接无效，请向朋友重新获取。");}}else setAddress(value);}} placeholder="服务器地址，或粘贴 edenvoice:// 邀请链接" aria-invalid={addressMissing} aria-describedby={addressMissing ? "address-error" : undefined} />{addressMissing && <span className="connect-field-error" id="address-error">请输入完整的服务器地址</span>}</label>
        {inviteError&&<p role="alert" className="inline-error">{inviteError}</p>}
        <label>昵称<input value={nickname} maxLength={64} onChange={(e) => setNickname(e.target.value)} placeholder="输入加入服务器时使用的昵称" aria-invalid={nicknameMissing} aria-describedby={nicknameMissing ? "nickname-error" : undefined} />{nicknameMissing && <span className="connect-field-error" id="nickname-error">请输入昵称</span>}</label>
        <label>服务器密码<input type="password" value={serverPassword} maxLength={256} autoComplete="off" onChange={e=>setServerPassword(e.target.value)} placeholder="无密码的服务器留空"/><small>仅用于本次连接与断线重连，不保存到磁盘。</small></label>
        <div className="remember-row"><div className="remember-options"><label><input type="checkbox" checked={remember} onChange={(e) => { setRemember(e.target.checked); if (!e.target.checked) setAutoConnect(false); }} />记住此服务器</label><label><input type="checkbox" checked={autoConnect} onChange={(e) => { setAutoConnect(e.target.checked); if (e.target.checked) setRemember(true); }} />启动时自动连接</label></div><button type="button" className="text-button" onClick={() => setIdentityOpen(true)}>管理</button></div>
        {snapshot.trustFingerprint && <div className="trust-card"><ShieldCheck size={18} /><div><strong>首次连接，需要确认服务器身份</strong><span>{snapshot.trustFingerprint}</span></div><button type="button" onClick={() => dispatch({ type: "trust_server", endpoint: snapshot.trustEndpoint, fingerprint: snapshot.trustFingerprint })}>信任并继续</button></div>}
        {snapshot.error && <div className="inline-error">{snapshot.error}</div>}
        <button className="primary connect-button" disabled={connecting}>{connecting ? <><span className="spinner" />正在连接</> : "进入服务器"}</button>
        <small>端到端身份验证 · TLS 1.3 控制连接</small>
      </form>{identityOpen && <IdentityManager snapshot={snapshot} close={() => setIdentityOpen(false)} />}
    </main>
  );
}

function ChannelRow({ channel, locked, unread, depth, users, active, social, onContext, onUserContext }: { channel: Channel; locked?: boolean; unread?: boolean; depth: number; users: User[]; active: boolean; social?: RoomSocialView; onContext: (event: MouseEvent, channel: Channel) => void; onUserContext: (event: MouseEvent, user: User) => void }) {
  const [expanded, setExpanded] = useState(true);
  const [memberPage,setMemberPage]=useState(0);
  const lastMemberPage=Math.max(0,Math.ceil(users.length/12)-1);
  const visibleMemberPage=Math.min(memberPage,lastMemberPage);
  return (
    <div className={`channel-block ${active ? "active" : ""}`}>
      <div className="channel-row" style={{ paddingLeft: 8 + depth * 14 }} onDoubleClick={() => dispatch({ type: "join_channel", channelId: channel.id })} onContextMenu={(e) => onContext(e, channel)}>
        <button className="disclosure" onClick={() => setExpanded(!expanded)}>{expanded ? <ChevronDown size={13} /> : <ChevronRight size={13} />}</button>
        {locked?<LockKeyhole size={14}/>:<Radio size={14}/>}<span className="channel-name">{channel.name}</span>{unread&&<i className="unread-dot" aria-label={`${channel.name} 有未读消息`}/>}
        {channel.temporary && <span className="mode-tag temporary">临时</span>}
        {roomAudioQualityBadge(channel.audioQualityPolicy) && <span className="mode-tag quality">{roomAudioQualityBadge(channel.audioQualityPolicy)}</span>}
        {channel.networkPolicy !== "auto" && <span className="mode-tag">{channel.networkPolicy === "force_p2p" ? "P2P" : "SFU"}</span>}
        {users.length > 0 && <span className="channel-count">{users.length}</span>}
      </div>
      {expanded && users.slice(visibleMemberPage*12,(visibleMemberPage+1)*12).map((user) => (
        <button className="voice-user" style={{ paddingLeft: 38 + depth * 14 }} key={user.sessionId} onClick={(event) => onUserContext(event, user)} onContextMenu={(event) => onUserContext(event, user)}>
          <Avatar avatar={user.avatar} name={user.nickname} size={22} speaking={user.speaking} />
          <span className="voice-user-name">{user.nickname}</span>
          {user.serverRole === "MASTER" && <Crown size={12} className="role-master" />}
          {user.serverRole === "ADMIN" && <ShieldCheck size={12} className="role-admin" />}
          {user.channelRole === "CHANNEL_ADMIN" && <Shield size={12} className="role-channel-admin" />}
          <span className="row-spacer" />
          <RoomMemberPresence sessionId={user.sessionId} nickname={user.nickname} social={active ? social : undefined}/>
          {user.speaking && <Signal size={12} className="speaking-icon" />}
          {user.deafened ? <HeadphoneOff size={12} className="muted-icon" /> : user.muted && <MicOff size={12} className="muted-icon" />}
        </button>
      ))}
      {expanded&&users.length>12&&<div className="channel-member-pages"><button disabled={visibleMemberPage===0} onClick={()=>setMemberPage(visibleMemberPage-1)} aria-label="上一页频道成员">‹</button><span>{visibleMemberPage+1}/{lastMemberPage+1} · {users.length} 人</span><button disabled={visibleMemberPage===lastMemberPage} onClick={()=>setMemberPage(visibleMemberPage+1)} aria-label="下一页频道成员">›</button></div>}
    </div>
  );
}

function Sidebar({ snapshot, currentChannel, social, onSettings, onServerMenu, onChannelContext, onUserContext, onCreate }: { snapshot: CoreSnapshot; currentChannel: string; social?: RoomSocialView; onSettings: () => void; onServerMenu: (event: MouseEvent) => void; onChannelContext: (event: MouseEvent, channel: Channel) => void; onUserContext: (event: MouseEvent, user: User) => void; onCreate: () => void }) {
  const unread=useUnreadRooms(snapshot);
  const children = useMemo(() => {
    const map = new Map<string, Channel[]>();
    snapshot.channels.forEach((channel) => map.set(channel.parentId, [...(map.get(channel.parentId) ?? []), channel]));
    map.forEach((items) => items.sort((a, b) => a.sortOrder - b.sortOrder));
    return map;
  }, [snapshot.channels]);
  const rows: Array<{ channel: Channel; depth: number }> = [];
  const append = (parent: string, depth: number) => (children.get(parent) ?? []).forEach((channel) => { rows.push({ channel, depth }); append(channel.id, depth + 1); });
  append("0", 0);
  const usersFor = (channelId: string) => snapshot.users.filter((user) => user.channelId === channelId);
  return (
    <aside className="sidebar">
      <button className="server-heading" aria-label="打开服务器菜单并切换服务器" onClick={onServerMenu}><div><small>当前服务器</small><strong>{snapshot.serverName || snapshot.serverAddress}</strong></div><ChevronDown size={16} /></button>
      <div className="channel-section-title"><button className="quick-jump-trigger" title="快捷跳转 Ctrl / Cmd + K" onClick={()=>window.dispatchEvent(new Event('eden-quick-jump'))}><Search size={12}/>快速跳转</button><span>语音频道</span>{snapshot.capabilities.includes("channel.create") && <button title="创建顶层频道" onClick={onCreate}><Plus size={14} /></button>}</div>
      <div className="channel-list">{rows.map(({ channel, depth }) => <ChannelRow key={channel.id} unread={unread[channel.id]} locked={snapshot.lockedChannelIds?.includes(channel.id)} channel={channel} depth={depth} users={usersFor(channel.id)} active={channel.id === currentChannel} social={social} onContext={onChannelContext} onUserContext={onUserContext} />)}</div>
      <div className="sidebar-profile">
        <Avatar avatar={snapshot.avatar} name={snapshot.nickname} size={34} speaking={snapshot.users.find((u) => u.sessionId === snapshot.selfSessionId)?.speaking} />
        <div className="profile-copy"><strong>{snapshot.nickname}</strong><span>{snapshot.serverRole || "USER"} · 在线</span></div>
        <button aria-label="打开设置" onClick={onSettings}><Settings size={17} /></button>
      </div>
    </aside>
  );
}

export function Chat({ snapshot, channel, roomSocial, membersVisible, onToggleMembers, onMore, onPlayerSettings }: { snapshot: CoreSnapshot; channel?: Channel; roomSocial?: ReturnType<typeof useRoomSocial>; membersVisible: boolean; onToggleMembers: () => void; onMore: (event: MouseEvent) => void; onPlayerSettings?: () => void }) {
  const {controller: socialController = null, social} = roomSocial ?? {};
  const [imageDraft, setImageDraft] = useState<{file: File; controller: RoomSocialController} | null>(null);
  const [imageNotice, setImageNotice] = useState('');
  const imageInput = useRef<HTMLInputElement>(null);
  const chooseImage = (file: File) => {
    if (!channel || !socialController?.imagesSupported) { setImageNotice('服务器尚未支持图片消息'); return; }
    setImageNotice(''); setImageDraft({file, controller: socialController});
  };
  useEffect(() => { setImageDraft(null); setImageNotice(''); }, [socialController]);
  const [roomVisible, setRoomVisibleState] = useState(true);
  const [screenVisible, setScreenVisible] = useState(false);
  const setRoomVisible=(value:boolean)=>{setScreenVisible(false);setRoomVisibleState(value);};
  useEffect(()=>{setScreenVisible(false);},[snapshot.serverAddress,snapshot.selfSessionId,channel?.id]);
  const [focusMessageId,setFocusMessageId]=useState("");
  const contextKey=roomKey(snapshot,channel?.id||'');
  const [text,setText]=useRoomDraft(contextKey);
  const [sendError, setSendError] = useState("");
  const [pendingMessage, setPendingMessage] = useState<{ text: string; draft: string; revision: number; knownIds: Set<string> } | null>(null);
  const sending = useRef(false);
  const sendSequence = useRef(0);
  const canSend = snapshot.connection === "connected" && !!channel;
  const textBytes = new TextEncoder().encode(text.trim()).length;
  const messageError = textBytes > 2048 ? "消息不能超过 2048 字节（约 682 个汉字），请分段发送。" : sendError;
  const [emojiOpen, setEmojiOpen] = useState(false);
  const [searchOpen, setSearchOpen] = useState(false);
  const [query, setQuery] = useState("");
  const searchInput = useRef<HTMLInputElement>(null);
  const composerInput = useRef<HTMLInputElement>(null);
  const composerSelection = useRef({ start: 0, end: 0 });
  const emojiPicker = useRef<HTMLDivElement>(null);
  const commonEmojis = ["😀", "😄", "😂", "😊", "😍", "🥰", "😎", "🤔", "😭", "😅", "😴", "😡", "👍", "👎", "👏", "🙏", "💪", "🤝", "🎉", "❤️", "🔥", "✨", "✅", "👀"];
  const history=useChatHistory(snapshot,channel?.id||'',query.trim());
  const liveMessages=snapshot.messages.filter(message=>message.channelId===channel?.id).slice(-100);
  const messageMap=new Map([...history.messages,...liveMessages].map(message=>[message.id,message]));
  const messages=[...messageMap.values()].sort(compareMessages).slice(-1000);
  useEffect(() => {
    sendSequence.current += 1;
    sending.current = false;
    setPendingMessage(null);
    setSendError("");
  }, [snapshot.serverAddress, channel?.id]);
  useEffect(() => {
    if (!pendingMessage) return;
    const delivered = messages.some(message => !pendingMessage.knownIds.has(message.id) && message.text === pendingMessage.text && isOwnChatMessage(message, snapshot.clientId));
    if (delivered) {
      setText(current => current === pendingMessage.draft ? "" : current);
      sending.current = false;
      setPendingMessage(null);
      setSendError("");
    } else if (!canSend || (snapshot.revision > pendingMessage.revision && snapshot.error)) {
      sending.current = false;
      setPendingMessage(null);
      setSendError(snapshot.error || "连接已中断，消息未确认发送，草稿已保留。");
    }
  }, [snapshot.revision, snapshot.connection, pendingMessage]);
  useEffect(() => {
    if (!pendingMessage) return;
    const timer = window.setTimeout(() => {
      sending.current = false;
      setPendingMessage(null);
      setSendError("尚未收到发送确认，草稿已保留。请检查聊天记录后重试。");
    }, 10000);
    return () => window.clearTimeout(timer);
  }, [pendingMessage]);
  const normalizedQuery = query.trim().toLocaleLowerCase();
  const visibleMessages = normalizedQuery ? messages.filter((message) => message.text.toLocaleLowerCase().includes(normalizedQuery) || message.senderName.toLocaleLowerCase().includes(normalizedQuery)) : messages;
  const chatScroll = useChatScroll(contextKey, messages.map((message) => message.id), normalizedQuery, !roomVisible&&!screenVisible);
  const restoredReading = useRef('');
  useEffect(()=>{restoredReading.current='';},[contextKey]);
  const recordReading = () => {
    const node=chatScroll.container.current;
    if(!node||restoredReading.current!==contextKey||roomVisible||screenVisible||normalizedQuery||document.hidden||!document.hasFocus())return;
    const rows=Array.from(node.querySelectorAll<HTMLElement>('[data-message-id]'));
    const bounds=node.getBoundingClientRect();
    const first=rows.find(row=>row.getBoundingClientRect().bottom>bounds.top);
    const atBottom=node.scrollHeight-node.clientHeight-node.scrollTop<=32;
    const visible=rows.filter(row=>row.getBoundingClientRect().bottom<=bounds.bottom+1&&row.getBoundingClientRect().bottom>bounds.top).at(-1)?.dataset.messageId;
    const read=atBottom?messages.at(-1)?.id:visible;
    const previous=roomPreferences(contextKey)?.read;
    updateRoom(contextKey,{...(first?{anchor:first.dataset.messageId}:{}),...(read&&newerMessage(read,previous)?{read}:{})});
  };
  useEffect(()=>{
    if(roomVisible||screenVisible||normalizedQuery||!messages.length)return;
    const frame=requestAnimationFrame(()=>{
      if(restoredReading.current!==contextKey){
        const anchor=roomPreferences(contextKey)?.anchor;
        const row=Array.from(chatScroll.container.current?.querySelectorAll<HTMLElement>('[data-message-id]')||[]).find(row=>row.dataset.messageId===anchor);
        if(anchor&&!row)return;
        if(row){row.scrollIntoView({block:'start'});chatScroll.onScroll();}
        restoredReading.current=contextKey;
      }
      recordReading();
    });
    window.addEventListener('focus',recordReading);document.addEventListener('visibilitychange',recordReading);
    return()=>{cancelAnimationFrame(frame);window.removeEventListener('focus',recordReading);document.removeEventListener('visibilitychange',recordReading);};
  },[contextKey,snapshot.revision,history.messages.length,roomVisible,screenVisible,normalizedQuery]);
  useEffect(()=>{if(roomVisible||!focusMessageId)return;const frame=requestAnimationFrame(()=>{const row=Array.from(chatScroll.container.current?.querySelectorAll<HTMLElement>('[data-message-id]')||[]).find(row=>row.dataset.messageId===focusMessageId);row?.scrollIntoView({block:'center'});row?.focus({preventScroll:true});});return()=>cancelAnimationFrame(frame);},[roomVisible,focusMessageId,history.messages.length]);
  useEffect(() => { if (searchOpen) searchInput.current?.focus(); }, [searchOpen]);
  useEffect(() => { setSearchOpen(false); setQuery(""); setEmojiOpen(false); composerSelection.current = { start: 0, end: 0 }; }, [snapshot.serverAddress, channel?.id]);
  useEffect(() => {
    if (!emojiOpen) return;
    const close = (event: PointerEvent) => { if (!emojiPicker.current?.contains(event.target as Node)) setEmojiOpen(false); };
    window.addEventListener("pointerdown", close);
    return () => window.removeEventListener("pointerdown", close);
  }, [emojiOpen]);
  const rememberComposerSelection = () => {
    const input = composerInput.current;
    if (input) composerSelection.current = { start: input.selectionStart ?? text.length, end: input.selectionEnd ?? text.length };
  };
  const insertEmoji = (emoji: string) => {
    const input = composerInput.current;
    const { start, end } = composerSelection.current;
    const next = `${text.slice(0, start)}${emoji}${text.slice(end)}`;
    if (next.length > 2048) return;
    setText(next);
    setEmojiOpen(false);
    requestAnimationFrame(() => {
      const position = Math.min(start + emoji.length, next.length);
      composerSelection.current = { start: position, end: position };
      input?.focus();
      input?.setSelectionRange(position, position);
    });
  };
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!canSend || sending.current || !text.trim() || textBytes > 2048) return;
    sending.current = true;
    const sequence = ++sendSequence.current;
    setSendError("");
    setPendingMessage({ text: text.trim(), draft: text, revision: snapshot.revision, knownIds: new Set(messages.map(message => message.id)) });
    setEmojiOpen(false);
    if (!await dispatch({ type: "send_text_message", text: text.trim() }) && sequence === sendSequence.current) {
      sending.current = false;
      setPendingMessage(null);
      setSendError("无法发送消息，草稿已保留，请重试。");
    }
  };
  return (
    <section className={`chat-panel has-room ${screenVisible ? "screen-mode" : roomVisible ? "room-mode" : ""} ${!channel ? "server-lounge" : ""}`}>
      <div className="chat-header">
        <div className="channel-symbol"><MessageCircle size={17} /></div>
        <div><h2>{channel?.name ?? "相聚之间"}</h2><p>{channel ? `${snapshot.users.filter((u) => u.channelId === channel.id).length} 人在语音频道` : `${snapshot.users.length} 位朋友在线 · 选个房间坐坐`}</p></div>
        <span className="chat-spacer" /><NotificationControls snapshot={snapshot} channel={channel?.id||''}/>
        {channel&&<div className="chat-view-switch"><button aria-pressed={roomVisible&&!screenVisible} onClick={()=>setRoomVisible(true)}>房间</button><button aria-pressed={!roomVisible&&!screenVisible} onClick={()=>setRoomVisible(false)}>聊天</button>{snapshot.screenShareSupported&&<button aria-pressed={screenVisible} onClick={()=>{setSearchOpen(false);setScreenVisible(true);}}>屏幕共享</button>}</div>}
        {searchOpen && <div className="chat-search"><Search size={14} /><input ref={searchInput} value={query} onKeyDown={event=>{if(event.key==='Enter'&&query.trim()){event.preventDefault();void history.load();}}} onChange={(event) => setQuery(event.target.value)} placeholder="搜索当前频道消息" /><span>{normalizedQuery ? `${visibleMessages.length} 条` : ""}</span><button aria-label="关闭搜索" onClick={() => { setSearchOpen(false); setQuery(""); }}><X size={13} /></button></div>}
        <button className={searchOpen ? "active" : ""} aria-label="搜索当前频道消息" title="搜索当前频道消息" onClick={() => {setRoomVisible(false);setSearchOpen((open) => !open);}}><Search size={18} /></button><button className={membersVisible ? "active" : ""} aria-label={membersVisible ? "隐藏服务器成员" : "显示服务器成员"} title={membersVisible ? "隐藏服务器成员" : "显示服务器成员"} onClick={onToggleMembers}><Users size={18} /></button><button aria-label="更多频道操作" title="更多频道操作" onClick={onMore}><MoreHorizontal size={18} /></button>
      </div>
      {channel && <ScreenShare key={JSON.stringify([snapshot.serverAddress,snapshot.selfSessionId,channel.id])} snapshot={snapshot} channelId={channel.id} selected={screenVisible} onSelect={()=>{setSearchOpen(false);setScreenVisible(true);}} onBack={()=>setRoomVisible(true)}/>}
      {<RoomExperience key={JSON.stringify([snapshot.serverAddress,snapshot.selfSessionId,channel?.id])} snapshot={snapshot} channelId={channel?.id||""} social={social} onAway={active=>{if(active)void socialController?.setAway(true);else void dispatch({type:"set_muted",muted:false});}} onReaction={emoji=>{void socialController?.react(emoji);}} visible={!channel||(roomVisible&&!screenVisible)} onReveal={()=>setRoomVisible(true)} onSettings={onPlayerSettings} onReadMessage={id=>{setQuery("");setSearchOpen(false);setFocusMessageId(id);setRoomVisible(false);}}/>}
      <div className="messages" ref={chatScroll.container} onScroll={()=>{chatScroll.onScroll();recordReading();const node=chatScroll.container.current;if(node&&node.scrollTop<24&&node.scrollHeight>node.clientHeight&&history.supported&&!normalizedQuery)void history.load();}}>
        {history.supported&&<div className="history-controls"><button disabled={history.busy||history.cursor===''} onClick={()=>void history.load()}>{history.busy?'正在查询…':history.cursor===''?(history.messages.length>=1000?'已加载 1000 条，请缩小搜索范围':'没有更早记录'):normalizedQuery?'搜索更早消息':'加载更早消息'}</button>{!normalizedQuery&&roomPreferences(contextKey)?.anchor&&!messages.some(message=>message.id===roomPreferences(contextKey)?.anchor)&&<><span>上次阅读位置在更早记录中</span><button onClick={()=>{restoredReading.current=contextKey;chatScroll.jumpToLatest();recordReading();}}>跳到最新消息</button></>}{history.error&&<p role="alert">{history.error}</p>}</div>}
        {visibleMessages.length === 0 ? <div className="empty-chat"><MessageCircle size={28} /><h3>{normalizedQuery ? "没有匹配的消息" : "这里还很安静"}</h3><p>{normalizedQuery ? history.supported&&history.cursor!=='' ? "可继续搜索更早记录，或换个关键词。" : "换个关键词再试试。" : "发条消息，和频道里的朋友打个招呼。"}</p></div> : visibleMessages.map((message) => (
          <article className={`message ${isOwnChatMessage(message, snapshot.clientId) ? "own" : ""}`} key={message.id} data-message-id={message.id} tabIndex={-1} data-highlighted={focusMessageId===message.id}>
            <Avatar avatar={message.senderAvatar} name={message.senderName} size={34} />
            <div><header><strong>{message.senderName}</strong><time>{chatTimestamp(message.sentAtUnixMs)}</time></header>{message.image ? <RoomImageAttachment image={message.image} controller={socialController} /> : <p>{message.text}</p>}</div>
          </article>
        ))}
      </div>
      {chatScroll.hasNewMessages && <button type="button" className="chat-new-messages" onClick={() => { setQuery(""); setSearchOpen(false); chatScroll.jumpToLatest(); }}>新消息 <ChevronDown size={14} /></button>}
      {imageNotice && <div className="composer-image-notice" role="status"><span>{imageNotice}</span>{imageNotice === '图片已发送' && <button type="button" onClick={()=>{setRoomVisible(false);setImageNotice('');}}>查看聊天</button>}<button type="button" aria-label="关闭图片提示" onClick={()=>setImageNotice('')}><X size={12}/></button></div>}
      {(messageError || pendingMessage || (channel && !canSend)) && <div className="composer-status" role={messageError ? "alert" : "status"}>{messageError || (pendingMessage ? "正在等待发送确认，草稿会保留到发送成功。" : "连接恢复中，草稿已保留，恢复后可发送。")}</div>}
      <form className="composer" onSubmit={submit} onPaste={event=>{const image = Array.from(event.clipboardData.files).find(file=>file.type.startsWith('image/'));if(image){event.preventDefault();chooseImage(image);}}}><input ref={composerInput} value={text} onChange={(e) => { setText(e.target.value); setSendError(""); composerSelection.current = { start: e.target.selectionStart ?? e.target.value.length, end: e.target.selectionEnd ?? e.target.value.length }; }} onSelect={rememberComposerSelection} onKeyUp={rememberComposerSelection} onClick={rememberComposerSelection} maxLength={2048} placeholder={channel ? `发消息到 ${channel.name}${socialController?.imagesSupported ? ' · 可粘贴截图' : ''}` : "加入频道后发送消息"} disabled={!channel} /><input type="file" ref={imageInput} hidden accept="image/png,image/jpeg,image/webp" aria-label="选择房间图片" onChange={event=>{const file=event.target.files?.[0];event.target.value='';if(file)chooseImage(file);}}/><button type="button" className="image-attach-button" aria-label="发送图片" title={socialController?.imagesSupported ? '发送图片，也可粘贴截图' : '服务器尚未支持图片消息'} disabled={!channel||!socialController?.imagesSupported} onClick={()=>imageInput.current?.click()}><ImagePlus size={18}/></button><div className="emoji-control" ref={emojiPicker}><button type="button" className={emojiOpen ? "active" : ""} aria-label="选择表情" title="选择表情" disabled={!channel} onMouseDown={(event) => { event.preventDefault(); rememberComposerSelection(); }} onClick={() => setEmojiOpen((open) => !open)}><Smile size={18} /></button>{emojiOpen && <div className="emoji-picker" role="dialog" aria-label="常用表情"><header>常用表情</header><div>{commonEmojis.map((emoji) => <button type="button" key={emoji} aria-label={`插入 ${emoji}`} onMouseDown={(event) => event.preventDefault()} onClick={() => insertEmoji(emoji)}>{emoji}</button>)}</div></div>}</div><button type="submit" className="composer-send" aria-label="发送消息" title="发送消息" disabled={!canSend || !!pendingMessage || !text.trim() || textBytes > 2048}><Send size={17} /></button></form>
      {imageDraft && imageDraft.controller === socialController && channel && <RoomImageComposer key={JSON.stringify([snapshot.serverAddress,snapshot.selfSessionId,channel.id,imageDraft.file.name,imageDraft.file.lastModified])} file={imageDraft.file} controller={socialController} roomName={channel.name} close={()=>setImageDraft(null)} sent={()=>{setImageDraft(null);setImageNotice('图片已发送');}}/>}
    </section>
  );
}

function MainMenu({ x, y, snapshot, close, onCompact, onNetworkDetails, onShortcuts, onSettings, onCreate, onBans, onMaster }: { x: number; y: number; snapshot: CoreSnapshot; close: () => void; onCompact: () => void; onNetworkDetails: () => void; onShortcuts: () => void; onSettings: () => void; onCreate: () => void; onBans: () => void; onMaster: () => void }) {
  const canCreate = snapshot.capabilities.includes("channel.create");
  const canViewBans = snapshot.capabilities.includes("member.ban_list");
  return <div className="context-menu main-menu" style={{ left: x, top: y }} onClick={(event) => event.stopPropagation()}><small>频道与服务器</small><button onClick={onCompact}><span>切换到缩略模式</span><Maximize2 size={13} /></button><button onClick={onNetworkDetails}>网络详情</button><button onClick={onShortcuts}>快捷键</button><button onClick={onSettings}>设置</button>{canCreate && <button onClick={onCreate}>创建顶层频道</button>}{canViewBans && <button onClick={onBans}>封禁管理</button>}{snapshot.serverRole !== "MASTER" && <button onClick={onMaster}>认领 MASTER</button>}<hr /><button className="danger-text" onClick={() => { dispatch({ type: "disconnect" }); close(); }}>断开连接</button></div>;
}

function NetworkDetails({ snapshot, channel, close }: { snapshot: CoreSnapshot; channel?: Channel; close: () => void }) {
  const network = connectionDisplay(snapshot);
  const members = channel ? snapshot.users.filter((user) => user.channelId === channel.id).length : 0;
  const quality = snapshot.networkQuality === "good" ? "良好" : snapshot.networkQuality || "检测中";
  return <div className="modal-backdrop compact-backdrop" onMouseDown={close}><section className="network-details" onMouseDown={(event) => event.stopPropagation()}><header><div><h2>网络详情</h2><p>当前控制连接、语音路由与设备状态</p></div><button aria-label="关闭网络详情" onClick={close}><X size={18} /></button></header><div className="network-details-body"><VoiceIncident/><div className="network-hero"><Signal size={21} /><div><strong>{network.label}</strong><span>{network.latency ?? "--"} ms {network.inVoice ? "语音" : "控制连接"} RTT · 网络{quality}</span></div><i /></div><Advice snapshot={snapshot}/><h3>连接</h3><dl><div><dt>语音路径 RTT</dt><dd>{network.inVoice ? `${snapshot.latencyMs ?? "--"} ms` : "未加入语音"}</dd></div><div><dt>控制连接 RTT</dt><dd>{snapshot.controlLatencyMs ?? "--"} ms</dd></div><div><dt>服务器</dt><dd>{snapshot.serverName || "未命名服务器"}</dd></div><div><dt>地址</dt><dd>{snapshot.serverAddress || "--"}</dd></div><div><dt>状态</dt><dd>{snapshot.connectionDetail || snapshot.connection}</dd></div><div><dt>频道</dt><dd>{channel?.name || "未加入"} · {members} 人</dd></div></dl><h3>音频</h3><dl><div><dt>麦克风</dt><dd>{snapshot.audio.selectedInput || "系统默认麦克风"}</dd></div><div><dt>扬声器</dt><dd>{snapshot.audio.selectedOutput || "系统默认输出"}</dd></div><div><dt>输入模式</dt><dd>{normalizeInputMode(snapshot.audio.inputMode) === "push_to_talk" ? "按键说话" : "语音活动检测"}</dd></div><div><dt>音频引擎</dt><dd>{snapshot.audio.state}</dd></div></dl></div><footer><button onClick={close}>完成</button></footer></section></div>;
}

function MemberPanel({ snapshot, onUserContext }: { snapshot: CoreSnapshot; onUserContext: (event: MouseEvent, user: User) => void }) {
  const [query,setQuery]=useState("");
  const [page,setPage]=useState(0);
  const members = snapshot.users.filter(user=>`${user.nickname} ${user.sessionId}`.toLocaleLowerCase().includes(query.toLocaleLowerCase()));
  const pages=Math.max(1,Math.ceil(members.length/25));const currentPage=Math.min(page,pages-1);
  return (
    <aside className="member-panel"><div className="member-title"><span>服务器成员</span><em>{snapshot.users.length}</em></div>
      <input className="server-member-search" aria-label="搜索服务器成员" placeholder="搜索成员" value={query} onChange={e=>{setQuery(e.target.value);setPage(0);}}/>
      {members.slice(currentPage*25,(currentPage+1)*25).map((user) => { const game = gameActivityLabel(user.gameActivity); return <button type="button" className="member-card" key={user.sessionId} title={`查看 ${user.nickname} 的成员操作`} onClick={(event) => onUserContext(event, user)} onContextMenu={(event) => onUserContext(event, user)}><Avatar avatar={user.avatar} name={user.nickname} size={32} /><div><div className="member-name"><strong>{user.nickname}</strong>{user.serverRole === "MASTER" && <Crown size={12} className="role-master" />}{user.serverRole === "ADMIN" && <ShieldCheck size={12} className="role-admin" />}</div><span title={game || "在线"}>{game || "在线"}</span></div><span className="row-spacer" />{(user.muted || user.adminMuted) && <MicOff size={12} className="muted-icon" />}{user.deafened && <HeadphoneOff size={12} className="muted-icon" />}</button>; })}
      {pages>1&&<div className="channel-member-pages"><button disabled={currentPage===0} onClick={()=>setPage(currentPage-1)}>上一页</button><span>{currentPage+1}/{pages}</span><button disabled={currentPage+1>=pages} onClick={()=>setPage(currentPage+1)}>下一页</button></div>}
    </aside>
  );
}

function DockAudioDevicePicker({ inputDevice, outputDevice, inputs, outputs, onInputChange, onOutputChange }: { inputDevice: string; outputDevice: string; inputs: string[]; outputs: string[]; onInputChange: (value: string) => void; onOutputChange: (value: string) => void }) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node)) setOpen(false); };
    const escape = (event: globalThis.KeyboardEvent) => { if (event.key === "Escape") setOpen(false); };
    window.addEventListener("pointerdown", close);
    window.addEventListener("keydown", escape);
    return () => { window.removeEventListener("pointerdown", close); window.removeEventListener("keydown", escape); };
  }, [open]);
  return <div className={`dock-device-picker ${open ? "open" : ""}`} ref={root}>
    <button type="button" className="dock-device-trigger" aria-haspopup="dialog" aria-expanded={open} aria-label="选择麦克风和扬声器" onClick={() => setOpen((current) => !current)}><Volume2 size={16} /><span>输入 / 输出设备</span><ChevronDown size={14} /></button>
    {open && <section className="dock-device-panel" role="dialog" aria-label="输入与输出设备"><header><div><strong>输入与输出设备</strong><small>分别选择麦克风和扬声器</small></div></header><label><span>麦克风</span><FlatSelect label="选择麦克风" value={inputDevice} options={audioDeviceOptions("系统默认麦克风", inputs)} onChange={onInputChange} /></label><label><span>扬声器</span><FlatSelect label="选择扬声器" value={outputDevice} options={audioDeviceOptions("系统默认输出", outputs)} onChange={onOutputChange} /></label></section>}
  </div>;
}

function VoiceDock({ snapshot, channel, onAudioSettings }: { snapshot: CoreSnapshot; channel?: Channel; onAudioSettings: () => void }) {
  const inputDevice = normalizeAudioDevice(snapshot.audio.selectedInput, "系统默认麦克风");
  const outputDevice = normalizeAudioDevice(snapshot.audio.selectedOutput, "系统默认输出");
  const inputMode = normalizeInputMode(snapshot.audio.inputMode);
  const audioPreferences = {
    inputDevice,
    outputDevice,
    inputGain: snapshot.audio.inputGain,
    outputVolume: snapshot.audio.outputVolume,
    inputMode,
    noiseSuppression: snapshot.audio.noiseSuppression,
    noiseSuppressionStrength: snapshot.audio.noiseSuppressionStrength,
    echoCancellation: snapshot.audio.echoCancellation,
    automaticGain: snapshot.audio.automaticGain,
  };
  const updateAudio = (updates: Partial<typeof audioPreferences>) => dispatch({ type: "update_audio_preferences", ...audioPreferences, ...updates });
  const updatePushToTalkShortcut = (pushToTalk: string) => dispatch({ type: "update_shortcuts", shortcuts: { ...snapshot.shortcuts, pushToTalk } });
  const selectInputDevice = (value: string) => updateAudio({ inputDevice: value });
  const selectOutputDevice = (value: string) => updateAudio({ outputDevice: value });
  const toggleMicrophone = () => dispatch({ type: "set_muted", muted: !snapshot.muted });
  const toggleSpeaker = () => dispatch({ type: "set_deafened", deafened: !snapshot.deafened });
  const microphoneLabel = snapshot.muted ? snapshot.deafened ? "打开麦克风与耳机" : "打开麦克风" : "关闭麦克风";
  const audioFailed = snapshot.audio.state.startsWith("error") || snapshot.audio.state === "stopped";
  const recovering = snapshot.connection !== "connected";
  const status = recovering ? "正在恢复连接" : channel ? audioFailed ? "语音不可用" : snapshot.audio.state === "ready" ? `已连接 · ${channel.name}` : "正在启动语音" : "尚未加入语音";
  return (
    <footer className="voice-dock">
      <div className={`voice-status ${audioFailed || recovering ? "unavailable" : ""}`}><span className="quality-dot" /><div><strong>{status}</strong><small>{recovering ? "恢复连接后可继续通话" : audioFailed ? "请检查设备与麦克风权限" : channel ? `${snapshot.transport} · ${snapshot.latencyMs ?? "--"} ms` : `控制连接 · ${snapshot.controlLatencyMs ?? "--"} ms`}</small></div>{audioFailed && !recovering && <button className="retry-audio" onClick={() => dispatch({ type: "retry_audio" })}>重试语音</button>}</div>
      <div className="voice-controls">
        <div className="voice-control-cluster microphone-cluster">
          <button className={`voice-control-button ${snapshot.muted ? "danger active" : ""}`} aria-label={microphoneLabel} title={microphoneLabel} onClick={toggleMicrophone}>{snapshot.muted ? <MicOff size={19} /> : <Mic size={19} />}<span className="level-ring" style={{ "--level": snapshot.inputLevel } as CSSProperties} /></button>
          <section className="voice-quick-panel microphone-quick-panel"><header><Mic size={14} /><strong>麦克风快捷设置</strong></header><label className="quick-range"><span>输入增益 <em>{Math.round(snapshot.audio.inputGain * 100)}%</em></span><input type="range" min="0" max="3" step="0.05" value={snapshot.audio.inputGain} onChange={(event) => updateAudio({ inputGain: Number(event.target.value) })} /></label><div className="quick-setting"><span>输入方式</span><div className="quick-segments"><button aria-pressed={inputMode === "voice_activity"} className={inputMode === "voice_activity" ? "selected" : ""} onClick={() => updateAudio({ inputMode: "voice_activity" })}>{inputMode === "voice_activity" && <Check size={11} />}语音激活</button><button aria-pressed={inputMode === "push_to_talk"} className={inputMode === "push_to_talk" ? "selected" : ""} onClick={() => updateAudio({ inputMode: "push_to_talk" })}>{inputMode === "push_to_talk" && <Check size={11} />}按键说话</button></div></div>{inputMode === "push_to_talk" && <div className="quick-setting quick-shortcut-binding"><span><span>说话按键</span><em>{snapshot.shortcuts.pushToTalk ? "已绑定" : "未绑定"}</em></span><ShortcutCapture value={snapshot.shortcuts.pushToTalk} onChange={updatePushToTalkShortcut} /><small>点击后按下单键或组合键，双击可清除</small></div>}<label className="quick-setting"><span>降噪方式 <InfoHint text={noiseModeHelp(snapshot.audio.noiseSuppression)} /></span><FlatSelect label="快捷选择降噪方式" value={noiseModeCategory(snapshot.audio.noiseSuppression)} onChange={(category) => updateAudio({ noiseSuppression: selectNoiseMode(category, snapshot.audio.noiseSuppression) })} options={NOISE_MODE_OPTIONS} /></label><button className="quick-settings-link" onClick={onAudioSettings}><Settings size={13} />完整语音设置</button></section>
        </div>
        <div className="voice-control-cluster speaker-cluster">
          <button className={`voice-control-button ${snapshot.deafened ? "danger active" : ""}`} aria-label={snapshot.deafened ? "恢复耳机声音" : "静音耳机并关闭麦克风"} title={snapshot.deafened ? "恢复耳机声音" : "静音耳机并关闭麦克风"} onClick={toggleSpeaker}>{snapshot.deafened ? <HeadphoneOff size={19} /> : <Headphones size={19} />}</button>
          <section className="voice-quick-panel speaker-quick-panel"><header><Headphones size={14} /><strong>耳机快捷设置</strong></header><label className="quick-range"><span>输出音量 <em>{Math.round(snapshot.audio.outputVolume * 100)}%</em></span><input type="range" min="0.01" max="3" step="0.01" value={snapshot.audio.outputVolume} onChange={(event) => updateAudio({ outputVolume: Number(event.target.value) })} /></label><button className="quick-settings-link" onClick={onAudioSettings}><Settings size={13} />完整语音设置</button></section>
        </div>
        <DockAudioDevicePicker inputDevice={inputDevice} outputDevice={outputDevice} inputs={snapshot.audio.inputs} outputs={snapshot.audio.outputs} onInputChange={selectInputDevice} onOutputChange={selectOutputDevice} />
        {channel && <button className="leave-channel" aria-label="离开当前频道" title="离开当前频道" onClick={() => dispatch({ type: "leave_channel" })}><LogOut size={19} /></button>}
      </div>
    </footer>
  );
}

function CompactTitleBar({ snapshot, expand }: { snapshot: CoreSnapshot; expand: () => void }) {
  const network = connectionDisplay(snapshot);
  const overlayLabel = snapshot.miniOverlayEnabled ? "关闭迷你悬浮窗" : "开启迷你悬浮窗";
  return <header className="compact-titlebar" data-tauri-drag-region><div className="compact-brand" data-tauri-drag-region><BrandIcon className="compact-brand-mark" /><strong>EdenVoice</strong></div><div className="compact-network" data-tauri-drag-region><Signal size={12} /><span>{network.label}</span><em>{network.latency ?? "--"} ms</em></div><div className="compact-window-actions"><FocusTimer/><button className={`compact-overlay-toggle ${snapshot.miniOverlayEnabled ? "active" : ""}`} aria-label={overlayLabel} title={overlayLabel} aria-pressed={snapshot.miniOverlayEnabled} onClick={() => dispatch({ type: "set_mini_overlay", enabled: !snapshot.miniOverlayEnabled, mode: snapshot.miniOverlayMode })}><PictureInPicture2 size={14} /><i /></button><button aria-label="恢复完整窗口" title="恢复完整窗口" onClick={expand}><Maximize2 size={14} /></button><button aria-label="最小化窗口" title="最小化窗口" onClick={() => windowAction("minimize")}><Minus size={14} /></button><button aria-label="关闭窗口" title="关闭窗口" onClick={() => windowAction("close")}><X size={14} /></button></div></header>;
}

function CompactView({ snapshot, channel, onUserContext }: { snapshot: CoreSnapshot; channel?: Channel; onUserContext: (event: MouseEvent, user: User) => void }) {
  const members = snapshot.users.filter((user) => user.channelId === channel?.id);
  return <main className="compact-view"><aside className="compact-channels"><div className="compact-section-label">语音频道</div>{snapshot.channels.map((item) => { const active = item.id === channel?.id; const count = snapshot.users.filter((user) => user.channelId === item.id).length; return <button className={active ? "active" : ""} key={item.id} onDoubleClick={() => dispatch({ type: "join_channel", channelId: item.id })}><Radio size={12} /><span>{item.name}</span>{count > 0 && <em>{count}</em>}</button>; })}</aside><section className="compact-members"><header><div><strong>{channel?.name || "尚未加入频道"}</strong><span>{members.length} 人在线</span></div></header><div className="compact-member-list">{members.length === 0 ? <p>频道内暂无成员</p> : members.map((user) => <button type="button" className="compact-member" key={user.sessionId} onClick={(event) => onUserContext(event, user)} onContextMenu={(event) => onUserContext(event, user)}><Avatar avatar={user.avatar} name={user.nickname} size={23} speaking={user.speaking} /><div><strong>{user.nickname}</strong><span>{user.speaking ? "正在说话" : user.muted || user.adminMuted ? "已静音" : "在线"}</span></div><span className="row-spacer" />{user.speaking && <Signal size={13} />}{(user.muted || user.adminMuted) && <MicOff size={13} />}</button>)}</div></section></main>;
}

type AudioSettingsProps = {
  snapshot: CoreSnapshot;
  inputDevice: string;
  setInputDevice: (value: string) => void;
  outputDevice: string;
  setOutputDevice: (value: string) => void;
  inputGain: number;
  setInputGain: (value: number) => void;
  outputVolume: number;
  setOutputVolume: (value: number) => void;
  inputMode: string;
  setInputMode: (value: string) => void;
  noiseSuppression: string;
  setNoiseSuppression: (value: string) => void;
  noiseSuppressionStrength: "low" | "medium" | "high";
  setNoiseSuppressionStrength: (value: "low" | "medium" | "high") => void;
  echoCancellation: boolean;
  setEchoCancellation: (value: boolean) => void;
  automaticGain: boolean;
  setAutomaticGain: (value: boolean) => void;
};

function AudioSettings(props: AudioSettingsProps) {
  const standardProcessing = props.noiseSuppression === "standard";
  const noiseCategory = noiseModeCategory(props.noiseSuppression);
  const apply = () => dispatch({
    type: "update_audio_preferences",
    inputDevice: props.inputDevice,
    outputDevice: props.outputDevice,
    inputGain: props.inputGain,
    outputVolume: props.outputVolume,
    inputMode: props.inputMode,
    noiseSuppression: props.noiseSuppression,
    noiseSuppressionStrength: props.noiseSuppressionStrength,
    echoCancellation: standardProcessing || props.echoCancellation,
    automaticGain: standardProcessing || props.automaticGain,
  });
  const test = (type: "play_output_test" | "start_mic_test" | "play_mic_test_original" | "play_mic_test_processed" | "stop_mic_test") => {
    if (type === "play_output_test" || type === "start_mic_test") apply();
    dispatch({ type });
  };
  const mic = props.snapshot.audio.micTestStatus;
  return <>
    <div className="settings-section-heading"><h3>输入与输出</h3><button title="重新扫描音频设备" onClick={() => dispatch({ type: "refresh_devices" })}><RefreshCw size={13} />刷新设备</button></div>
    <div className="field-grid"><label>麦克风<FlatSelect label="麦克风" value={props.inputDevice} onChange={props.setInputDevice} options={audioDeviceOptions("系统默认麦克风", props.snapshot.audio.inputs)} /></label><label>扬声器<FlatSelect label="扬声器" value={props.outputDevice} onChange={props.setOutputDevice} options={audioDeviceOptions("系统默认输出", props.snapshot.audio.outputs)} /></label></div>
    <div className="audio-sample-rate-note"><Info size={13} /><span>EdenVoice 内部统一使用 48 kHz；设备采样率会在输入输出边界自动适配。</span></div>
    <div className="field-grid audio-mode-row"><label>输入模式<FlatSelect label="输入模式" value={props.inputMode} onChange={props.setInputMode} options={[{ value: "voice_activity", label: "语音活动检测" }, { value: "push_to_talk", label: "按键说话" }]} /></label><button className="audio-test-button" onClick={() => test("play_output_test")} disabled={props.snapshot.audio.state !== "ready"}>播放输出测试音</button></div>
    <label className="range-field"><span>输入增益 <em>{Math.round(props.inputGain * 100)}%</em></span><input type="range" min="0" max="3" step="0.05" value={props.inputGain} onChange={(e) => props.setInputGain(Number(e.target.value))} /></label>
    <label className="range-field"><span>输出音量 <em>{Math.round(props.outputVolume * 100)}%</em></span><input type="range" min="0.01" max="3" step="0.01" value={props.outputVolume} onChange={(e) => props.setOutputVolume(Number(e.target.value))} /></label>
    <div className="mic-test-card"><div><strong>麦克风自听测试</strong><span>手动开始和停止，最长录制 10 秒；可对比原声与处理后声音，不会上传。</span></div><div className="mic-test-actions">
      {mic === "idle" && <button onClick={() => test("start_mic_test")} disabled={props.snapshot.audio.state !== "ready"}>开始录音</button>}
      {mic === "recording" && <><em>正在录音（最长 10 秒）…</em><button onClick={() => test("stop_mic_test")}>停止并生成试听</button></>}
      {mic === "preview_ready" && <><button onClick={() => test("play_mic_test_original")}>播放原声</button><button onClick={() => test("play_mic_test_processed")}>播放处理后</button><button onClick={() => test("stop_mic_test")}>清除</button></>}
      {(mic === "playing_original" || mic === "playing_processed") && <><em>{mic === "playing_original" ? "正在播放原声…" : "正在播放处理后…"}</em><button onClick={() => test("stop_mic_test")}>停止</button></>}
    </div></div>
    {(props.snapshot.audio.deviceError || props.snapshot.audio.dspError) && <div className="audio-error">{props.snapshot.audio.deviceError || props.snapshot.audio.dspError}</div>}
    <h3>语音处理</h3><div className="field-grid"><label><span className="field-label-with-help">降噪模式 <InfoHint text={noiseModeHelp(props.noiseSuppression)} /></span><FlatSelect label="降噪模式" value={noiseCategory} onChange={(category) => props.setNoiseSuppression(selectNoiseMode(category, props.noiseSuppression))} options={NOISE_MODE_OPTIONS} /></label>{noiseCategory === "standard" && <div className="noise-strength-field"><span>日常降噪强度</span><div className="quick-segments">{([['low', '轻度'], ['medium', '适中'], ['high', '强力']] as const).map(([value, label]) => <button type="button" aria-pressed={props.noiseSuppressionStrength === value} className={props.noiseSuppressionStrength === value ? "selected" : ""} key={value} onClick={() => props.setNoiseSuppressionStrength(value)}>{props.noiseSuppressionStrength === value && <Check size={11} />}{label}</button>)}</div></div>}{noiseCategory === "voice_isolation" && <div className="noise-strength-field"><span>人声隔离效果</span><div className="quick-segments two-options">{([['deepfilternet', '自然'], ['deepfilternet_strong', '强力']] as const).map(([value, label]) => <button type="button" aria-pressed={props.noiseSuppression === value} className={props.noiseSuppression === value ? "selected" : ""} key={value} onClick={() => props.setNoiseSuppression(value)}>{props.noiseSuppression === value && <Check size={11} />}{label}</button>)}</div></div>}<small className="field-hint">{noiseCategory === "voice_isolation" ? "自然优先保留音质；强力优先隔离复杂环境声。" : ""}</small></div>
    <div className={`setting-row ${standardProcessing ? "built-in" : ""}`}><div><strong>回声消除</strong><span>{standardProcessing ? "标准模式已内置并自动启用，无需单独开启" : "减少扬声器声音被麦克风重复采集"}</span></div><input type="checkbox" checked={standardProcessing || props.echoCancellation} disabled={standardProcessing} onChange={(e) => props.setEchoCancellation(e.target.checked)} /></div><div className={`setting-row ${standardProcessing ? "built-in" : ""}`}><div><strong>自动增益</strong><span>{standardProcessing ? "标准模式已内置并自动启用，无需单独开启" : "自动保持稳定的人声响度"}</span></div><input type="checkbox" checked={standardProcessing || props.automaticGain} disabled={standardProcessing} onChange={(e) => props.setAutomaticGain(e.target.checked)} /></div><div className="setting-note"><Wifi size={16} /><span>当前 {props.snapshot.transport} · {props.snapshot.latencyMs ?? "--"} ms，网络状态{props.snapshot.networkQuality === "good" ? "良好" : props.snapshot.networkQuality}</span></div>
  </>;
}

function DataTransferDialog({ mode, close }: { mode: "export" | "import"; close: () => void }) {
  const [password, setPassword] = useState("");
  const [confirmation, setConfirmation] = useState("");
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState("");
  const exporting = mode === "export";
  const valid = Array.from(password).length >= 10 && (!exporting || password === confirmation);
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!valid || busy) return;
    setBusy(true);
    setStatus("");
    try {
      if (exporting) {
        const path = await exportUserData(password);
        setStatus(path ? `加密备份已保存到 ${path}` : "已取消导出");
        if (path) {
          setPassword("");
          setConfirmation("");
        }
      } else {
        const clientId = await importUserData(password);
        if (clientId) {
          setPassword("");
          setStatus("导入成功，客户端即将重启并应用全部数据…");
          window.setTimeout(() => void restartClient(), 1200);
        } else {
          setStatus("已取消导入");
        }
      }
    } catch (error) {
      setStatus(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };
  return <div className="transfer-dialog-backdrop" onMouseDown={() => { if (!busy) close(); }}><form className="data-transfer-dialog" role="dialog" aria-modal="true" aria-label={exporting ? "导出加密备份" : "导入加密备份"} onSubmit={submit} onMouseDown={(event) => event.stopPropagation()}>
    <header><div><h3>{exporting ? "导出加密备份" : "导入加密备份"}</h3><p>{exporting ? "用密码保护个人资料、设置、Client ID 与服务器记录。" : "导入会替换本机数据，成功后客户端将自动重启。"}</p></div><button type="button" onClick={close} disabled={busy} aria-label="关闭"><X size={17} /></button></header>
    <div className="data-transfer-fields">
      <label>{exporting ? "设置备份密码" : "备份密码"}<input autoFocus type="password" minLength={10} maxLength={1024} autoComplete={exporting ? "new-password" : "current-password"} value={password} onChange={(event) => setPassword(event.target.value)} placeholder="至少 10 个字符" /></label>
      {exporting && <label>确认密码<input type="password" minLength={10} maxLength={1024} autoComplete="new-password" value={confirmation} onChange={(event) => setConfirmation(event.target.value)} placeholder="再次输入密码" /></label>}
      {exporting && confirmation && password !== confirmation && <p className="transfer-error">两次输入的密码不一致</p>}
      <p className="transfer-security-note"><ShieldCheck size={15} />{exporting ? "密码不会保存；忘记密码后无法恢复备份。文件使用 Argon2id 与 XChaCha20-Poly1305 加密。" : "密码只用于本次本地解密。密码错误或文件被修改时不会覆盖现有数据。"}</p>
      {status && <p className={`transfer-status ${status.includes("失败") || status.includes("错误") ? "error" : ""}`} role="status">{status}</p>}
    </div>
    <footer><button type="button" onClick={close} disabled={busy}>取消</button><button className="primary" type="submit" disabled={!valid || busy}>{busy ? "处理中…" : exporting ? "选择位置并导出" : "选择备份并导入"}</button></footer>
  </form></div>;
}

export function SettingsModal({ snapshot, close, initialPage = "audio" }: { snapshot: CoreSnapshot; close: () => void; initialPage?: SettingsPage }) {
  const [page, setPage] = useState<SettingsPage>(initialPage);
  const [avatarSource, setAvatarSource] = useState("");
  const [displayName, setDisplayName] = useState(snapshot.nickname);
  const [clientIdCopied, setClientIdCopied] = useState(false);
  const [transferMode, setTransferMode] = useState<"export" | "import" | null>(null);
  const [inputDevice, setInputDevice] = useState(normalizeAudioDevice(snapshot.audio.selectedInput, "系统默认麦克风"));
  const [outputDevice, setOutputDevice] = useState(normalizeAudioDevice(snapshot.audio.selectedOutput, "系统默认输出"));
  const [inputGain, setInputGain] = useState(snapshot.audio.inputGain);
  const [outputVolume, setOutputVolume] = useState(snapshot.audio.outputVolume);
  const [inputMode, setInputMode] = useState(normalizeInputMode(snapshot.audio.inputMode));
  const [noiseSuppression, setNoiseSuppression] = useState(snapshot.audio.noiseSuppression);
  const [noiseSuppressionStrength, setNoiseSuppressionStrength] = useState<"low" | "medium" | "high">(snapshot.audio.noiseSuppressionStrength);
  const [echoCancellation, setEchoCancellation] = useState(snapshot.audio.echoCancellation);
  const [automaticGain, setAutomaticGain] = useState(snapshot.audio.automaticGain);
  const [theme, setTheme] = useState(snapshot.theme);
  const [miniOverlayEnabled, setMiniOverlayEnabled] = useState(snapshot.miniOverlayEnabled);
  const [miniOverlayMode, setMiniOverlayMode] = useState<"avatars" | "list">(snapshot.miniOverlayMode === "list" ? "list" : "avatars");
  const [archiveEnabled, setArchiveEnabled] = useState(() => localStorage.getItem("edenArchiveEnabled") !== "false");
  const [autoConnectLastServer, setAutoConnectLastServer] = useState(snapshot.autoConnectLastServer);
  const [autoConnectServer, setAutoConnectServer] = useState(snapshot.savedServers.some((profile) => profile.address === snapshot.autoConnectServer) ? snapshot.autoConnectServer : snapshot.savedServers[0]?.address || "");
  const [shortcuts, setShortcuts] = useState<ShortcutState>(snapshot.shortcuts);
  const savedServerKey = JSON.stringify(snapshot.savedServers.map((profile) => profile.address));
  useEffect(() => setAutoConnectLastServer(snapshot.autoConnectLastServer), [snapshot.autoConnectLastServer]);
  useEffect(() => {
    setAutoConnectServer((current) => snapshot.savedServers.some((profile) => profile.address === current) ? current : snapshot.savedServers.some((profile) => profile.address === snapshot.autoConnectServer) ? snapshot.autoConnectServer : snapshot.savedServers[0]?.address || "");
  }, [savedServerKey, snapshot.autoConnectServer]);
  const setShortcut = (key: keyof ShortcutState, value: string) => setShortcuts((current) => ({ ...current, [key]: value }));
  const copyClientId = async () => {
    if (!snapshot.clientId) return;
    let copied = false;
    try {
      await navigator.clipboard.writeText(snapshot.clientId);
      copied = true;
    } catch {
      const field = document.createElement("textarea");
      field.value = snapshot.clientId;
      field.style.position = "fixed";
      field.style.opacity = "0";
      document.body.appendChild(field);
      field.select();
      copied = document.execCommand("copy");
      field.remove();
    }
    if (copied) {
      setClientIdCopied(true);
      window.setTimeout(() => setClientIdCopied(false), 1600);
    }
  };
  const save = () => {
    dispatch({
      type: "update_audio_preferences",
      inputDevice,
      outputDevice,
      inputGain,
      outputVolume,
      inputMode,
      noiseSuppression,
      noiseSuppressionStrength,
      echoCancellation,
      automaticGain,
    });
    dispatch({ type: "update_shortcuts", shortcuts });
    dispatch({ type: "set_theme", theme });
    dispatch({ type: "set_mini_overlay", enabled: miniOverlayEnabled, mode: miniOverlayMode });
    dispatch({ type: "set_auto_connect", enabled: autoConnectLastServer, serverAddress: autoConnectServer });
    localStorage.setItem("edenArchiveEnabled", String(archiveEnabled));
    if (displayName.trim()) dispatch({ type: "set_display_name", displayName: displayName.trim() });
    close();
  };
  return <div className="modal-backdrop" onMouseDown={close}><div className="settings-modal" onMouseDown={(e) => e.stopPropagation()}>
    <header><div><h2>设置</h2><p>设备、声音和界面偏好</p></div><button onClick={close}><X size={19} /></button></header>
    <div className="settings-body"><nav><button className={page === "player" ? "active" : ""} onClick={() => setPage("player")}><Settings size={16} />播放器</button><button className={page === "profile" ? "active" : ""} onClick={() => setPage("profile")}><CircleUserRound size={16} />个人资料</button><button className={page === "audio" ? "active" : ""} onClick={() => setPage("audio")}><Volume2 size={16} />音频</button><button className={page === "diagnostics" ? "active" : ""} onClick={() => setPage("diagnostics")}><Activity size={16} />语音诊断</button><button className={page === "servers" ? "active" : ""} onClick={() => setPage("servers")}><Server size={16} />服务器</button><button className={page === "appearance" ? "active" : ""} onClick={() => setPage("appearance")}><Palette size={16} />界面</button><button className={page === "shortcuts" ? "active" : ""} onClick={() => setPage("shortcuts")}><Radio size={16} />快捷键</button><button className={page === "about" ? "active" : ""} onClick={() => setPage("about")}><Info size={16} />关于</button></nav>
      <section className="settings-content">
        {page === "player" && <PlayerSettings />}
        {page === "profile" && <><h3>个人资料</h3><p className="section-description">名称与头像保存在本机，连接服务器时会作为默认资料使用。</p><div className="profile-settings-card"><Avatar avatar={snapshot.avatar} name={displayName} size={68} /><div className="profile-settings-fields"><label>昵称<input value={displayName} maxLength={64} onChange={(event) => setDisplayName(event.target.value)} placeholder="输入昵称" /></label><span>未上传头像时，自动使用名称的第一个字作为头像。</span></div><div className="profile-avatar-actions"><label className="upload-avatar"><Upload size={14} />更换头像<input type="file" accept="image/png,image/jpeg,image/webp,image/gif" onChange={(event) => { const file = event.target.files?.[0]; if (!file || file.size > 20 * 1024 * 1024) return; const reader = new FileReader(); reader.onload = () => setAvatarSource(String(reader.result || "")); reader.readAsDataURL(file); event.currentTarget.value = ""; }} /></label>{snapshot.avatar && <button type="button" className="remove-avatar" onClick={() => dispatch({ type: "set_avatar", avatar: "" })}>恢复默认</button>}</div></div><h3>数据迁移</h3><p className="section-description">在不同电脑之间迁移个人资料、设置、Client ID、头像和服务器记录。</p><div className="data-transfer-card"><button type="button" onClick={() => setTransferMode("export")}><Download size={16} /><span><strong>导出数据</strong><small>设置密码并创建加密备份</small></span></button><button type="button" onClick={() => setTransferMode("import")}><Upload size={16} /><span><strong>导入数据</strong><small>输入密码并恢复加密备份</small></span></button></div></>}
        {page === "audio" && <AudioSettings snapshot={snapshot} inputDevice={inputDevice} setInputDevice={setInputDevice} outputDevice={outputDevice} setOutputDevice={setOutputDevice} inputGain={inputGain} setInputGain={setInputGain} outputVolume={outputVolume} setOutputVolume={setOutputVolume} inputMode={inputMode} setInputMode={setInputMode} noiseSuppression={noiseSuppression} setNoiseSuppression={setNoiseSuppression} noiseSuppressionStrength={noiseSuppressionStrength} setNoiseSuppressionStrength={setNoiseSuppressionStrength} echoCancellation={echoCancellation} setEchoCancellation={setEchoCancellation} automaticGain={automaticGain} setAutomaticGain={setAutomaticGain} />}
        {page === "diagnostics" && <DiagnosticsSettings snapshot={snapshot} onAction={action=>setPage(action==='audio'?'audio':'diagnostics')} />}
        {page === "servers" && <><h3>启动行为</h3><div className="setting-row"><div><strong>启动时自动连接</strong><span>启动后连接指定服务器；失败时返回连接页</span></div><input type="checkbox" checked={autoConnectLastServer} onChange={(event) => setAutoConnectLastServer(event.target.checked)} /></div>{snapshot.savedServers.length > 0 ? <label className={`auto-connect-server-select ${autoConnectLastServer ? "" : "disabled"}`}><span>自动连接服务器</span><FlatSelect label="自动连接服务器" value={autoConnectServer} options={snapshot.savedServers.map((profile) => ({ value: profile.address, label: profile.displayName || profile.serverName || profile.address }))} onChange={setAutoConnectServer} disabled={!autoConnectLastServer} /><small>{snapshot.savedServers.length > 1 ? "存在多个已保存服务器时，可在这里指定启动目标。" : "当前只有一台已保存服务器。"}</small></label> : <p className="setting-note"><Info size={14} />连接并记住一台服务器后，才可以指定自动连接目标。</p>}<h3>服务器</h3><SavedServerSettings snapshot={snapshot} /></>}
        {page === "profile" && <><h3>游戏状态</h3><div className="setting-row"><div><strong>共享正在玩的游戏</strong><span>仅 Windows：每 5 秒识别支持的游戏，只向当前服务器分享游戏名称。关闭后立即撤回。</span></div><input aria-label="共享正在玩的游戏" type="checkbox" disabled={!snapshot.gameDetectionAvailable} checked={!!snapshot.shareGameActivity} onChange={event=>dispatch({type:"set_share_game_activity",enabled:event.target.checked})}/></div><p className="setting-note">{!snapshot.gameDetectionAvailable?"此设备不支持 Windows 游戏识别，但可以查看朋友分享的状态。":!snapshot.gameActivitySupported?"当前服务器尚未支持游戏状态；升级服务器后生效。":snapshot.shareGameActivity?(snapshot.users.find(user=>user.sessionId===snapshot.selfSessionId)?.gameActivity?.title?`正在分享：${snapshot.users.find(user=>user.sessionId===snapshot.selfSessionId)?.gameActivity?.title}`:"等待识别支持的游戏；启动器和未知程序不会展示。"):"游戏状态分享已关闭。此开关立即保存并生效。"}</p></>}
        {page === "appearance" && <div className="setting-row archive-setting"><div><strong>Eden Archive</strong><span>在房间菜单显示档案入口。仅记录参与、时间与 speaking 元数据，不录制语音。</span></div><input type="checkbox" checked={archiveEnabled} onChange={(event) => setArchiveEnabled(event.target.checked)} /></div>}
        {page === "appearance" && <><CloseBehaviorSettings /><h3>外观</h3><div className="theme-options">{([['dark', '深色'], ['light', '浅色'], ['system', '跟随系统']] as const).map(([value, label]) => <button key={value} className={theme === value ? "selected" : ""} onClick={() => setTheme(value)}><span className={`theme-preview ${value}`} />{label}</button>)}</div><h3>迷你悬浮窗</h3><div className="mini-overlay-settings"><div className="setting-row"><div><strong>显示迷你悬浮窗</strong><span>连接频道后置于其他窗口上方，可拖动到屏幕任意位置</span></div><input type="checkbox" checked={miniOverlayEnabled} onChange={(event) => setMiniOverlayEnabled(event.target.checked)} /></div><div className={`mini-mode-options ${miniOverlayEnabled ? "" : "disabled"}`}>{([['avatars', '头像模式'], ['list', '列表模式']] as const).map(([mode, label]) => <button type="button" key={mode} disabled={!miniOverlayEnabled} className={miniOverlayMode === mode ? "selected" : ""} onClick={() => setMiniOverlayMode(mode)}><span className={`mini-mode-preview ${mode}`}>{mode === "avatars" ? <><i /><i /><i /></> : <><i /><i /><i /></>}</span><strong>{label}</strong><small>{mode === "avatars" ? "横向显示，适合游戏画面" : "显示名称，状态更清晰"}</small></button>)}</div></div></>}
        {page === "shortcuts" && <><h3>快捷键</h3><p className="section-description">默认不绑定。点击按键框录入单键或 0.5 秒内完成的组合键，双击清除。</p>{([
          ["toggleWindow", "切换小窗 / 大窗口"],
          ["toggleMic", "切换麦克风"],
          ["toggleSpeaker", "切换扬声器"],
          ["pushToTalk", "按键说话"],
          ["toggleMembers", "显示 / 隐藏成员列表"],
        ] as const).map(([key, label]) => <div className="shortcut-row" key={key}><span>{label}</span><ShortcutCapture value={shortcuts[key]} onChange={(value) => setShortcut(key, value)} /></div>)}</>}
        {page === "about" && <div className="about-settings"><div className="about-product"><BrandIcon className="about-logo" /><div><h3>EdenVoice</h3><p>轻量、稳定的语音协作客户端</p><span>Desktop v0.8.5</span></div></div><section className="client-identity-card"><header><div><strong>本机 Client ID</strong><span>用于服务器识别这台客户端及分配管理权限</span></div></header><div className="client-id-row"><code>{snapshot.clientId || "身份初始化失败"}</code><button type="button" onClick={copyClientId} disabled={!snapshot.clientId} aria-label="复制 Client ID">{clientIdCopied ? <Check size={14} /> : <Copy size={14} />}<span>{clientIdCopied ? "已复制" : "复制"}</span></button></div><p>Client ID 由本机身份公钥生成，不包含私钥。重装或删除客户端身份文件后会发生变化。</p></section><UpdateNotice settings/><footer className="about-credit"><span>Designed &amp; built by</span><strong>SmileSion</strong></footer></div>}
      </section></div>
    <footer><button onClick={close}>取消</button><button className="primary" onClick={save}>保存设置</button></footer>
    {avatarSource && <AvatarEditor source={avatarSource} close={() => setAvatarSource("")} />}
    {transferMode && <DataTransferDialog mode={transferMode} close={() => setTransferMode(null)} />}
  </div></div>;
}

function ChannelEditor({ mode, channel, channels, close, passwordSupported }: { mode: "top" | "child" | "edit"; channel?: Channel; channels: Channel[]; passwordSupported?: boolean; close: () => void }) {
  const [passwordAction,setPasswordAction]=useState("keep"),[roomPassword,setRoomPassword]=useState("");
  const [name, setName] = useState(mode === "edit" ? channel?.name ?? "" : "");
  const [userLimit, setUserLimit] = useState(mode === "edit" ? channel?.userLimit ?? 0 : 0);
  const [audioQualityPolicy, setAudioQualityPolicy] = useState(mode === "edit" ? normalizeRoomAudioQuality(channel?.audioQualityPolicy ?? "auto") : "auto");
  const [temporary, setTemporary] = useState(mode === "edit" ? channel?.temporary ?? false : false);
  const saveChannel = (event: FormEvent) => {
    event.preventDefault();
    if (!name.trim()) return;
    const parentId = mode === "child" ? channel?.id ?? "0" : mode === "edit" ? channel?.parentId ?? "0" : "0";
    const sortOrder = mode === "edit" ? channel?.sortOrder ?? 0 : mode === "child" ? (channel?.sortOrder ?? 0) + 1 : Math.max(0, ...channels.filter((item) => item.parentId === "0").map((item) => item.sortOrder)) + 1;
    const next: Channel = { id: mode === "edit" ? channel?.id ?? "0" : "0", parentId, name: name.trim(), sortOrder, userLimit: Math.min(10_000, Math.max(0, userLimit)), networkPolicy: channel?.networkPolicy ?? "auto", audioQualityPolicy, temporary };
    dispatch({ type: mode === "edit" ? "edit_channel" : "create_channel", channel: next, ...(mode!=="edit"&&passwordAction==="set"?{password:roomPassword}:{}) });
    if(mode==="edit"&&channel&&passwordAction!=="keep")void dispatch({type:"set_channel_password",channelId:channel.id,password:passwordAction==="remove"?"":roomPassword});
    close();
  };
  return <div className="modal-backdrop compact-backdrop" onMouseDown={close}><form className="channel-editor channel-editor-expanded" onSubmit={saveChannel} onMouseDown={(event) => event.stopPropagation()}>
    <header><div><h2>{mode === "edit" ? "编辑频道" : mode === "child" ? `在 ${channel?.name} 下创建子频道` : "创建顶层频道"}</h2><p>音质由房间统一控制；临时房间在最后一人离开后自动删除。</p></div><button type="button" onClick={close}><X size={18} /></button></header>
    <div className="channel-editor-fields expanded"><label>频道名称<input autoFocus value={name} maxLength={64} onChange={(event) => setName(event.target.value)} placeholder="例如：开黑小队" /></label><label>人数上限<input type="number" min="0" max="10000" value={userLimit} onChange={(event) => setUserLimit(Number(event.target.value))} /><small>0 表示不限制</small></label><div className="room-quality-field"><span>房间音频质量</span><RoomAudioQualityPicker value={audioQualityPolicy} onChange={setAudioQualityPolicy} /><small>等级 1–10 对应 16–96 kbps；自动模式会随网络调整</small></div>{<label>进入房间密码<select disabled={!passwordSupported} aria-label="房间密码操作" value={passwordAction} onChange={e=>setPasswordAction(e.target.value)}><option value="keep">{mode==="edit"?"保持现有设置":"无需密码"}</option><option value="set">设置 / 修改密码</option><option value="remove">取消密码</option></select>{!passwordSupported&&<small>当前服务器需升级后才能设置密码</small>}{passwordAction==="set"&&<input aria-label="新房间密码" type="password" maxLength={72} value={roomPassword} onChange={e=>setRoomPassword(e.target.value)} autoComplete="new-password"/>}</label>}<label className="temporary-channel-toggle"><span><strong>临时房间</strong><small>{temporary ? "创建后自动进入，空房立即删除" : "永久保存到服务器"}</small></span><input type="checkbox" checked={temporary} onChange={(event) => setTemporary(event.target.checked)} /></label></div>
    <footer><button type="button" onClick={close}>取消</button><button className="primary" type="submit" disabled={!name.trim()||(passwordAction==="set"&&!roomPassword)}>保存频道</button></footer>
  </form></div>;
}

function DeleteChannelConfirm({ channel, close }: { channel: Channel; close: () => void }) {
  return <div className="modal-backdrop compact-backdrop" onMouseDown={close}><section className="delete-confirm" onMouseDown={(event) => event.stopPropagation()}><h2>删除“{channel.name}”？</h2><p>存在子频道时服务端会拒绝删除；频道内用户将由服务端移动到回退频道。</p><footer><button onClick={close}>取消</button><button className="delete-button" onClick={() => { dispatch({ type: "delete_channel", channelId: channel.id }); close(); }}>确认删除</button></footer></section></div>;
}

const archiveCopy: Record<string, { zh: [string, string]; en: [string, string] }> = {
  first_room_created: { zh: ["房间的起点", "从这一天起，这里开始留下痕迹。"], en: ["The Beginning", "This room began leaving traces here."] },
  first_member_join: { zh: ["第一次相遇", "第一次有人走进这个房间。"], en: ["First Arrival", "Someone entered this room for the first time."] },
  first_voice: { zh: ["First Voice", "这里第一次有了声音。"], en: ["First Voice", "A voice was heard here for the first time."] },
  first_text_message: { zh: ["第一条消息", "有人在安静中留下了第一句话。"], en: ["First Message", "The first words were left in the quiet."] },
  last_two_5m: { zh: ["Only Two", "其他人离开后，两个人留了下来。"], en: ["Only Two", "The others had gone. Two remained."] },
  last_two_30m: { zh: ["They Were Only Two", "其他人都离开后，两个人又待了很久。"], en: ["They Were Only Two", "After everyone left, two stayed a while longer."] },
  last_two_1h: { zh: ["只剩两道声音", "整整一小时，只有两道声音留在这里。"], en: ["Two Voices Remained", "Only two voices remained for an hour."] },
  eden_awakened: { zh: ["Welcome Back", "这里安静了很久。今天，声音又回来了。"], en: ["Welcome Back", "It was quiet here for a long time. Today, voices returned."] },
  still_here_at_3am: { zh: ["凌晨三点", "夜已经很深了，但这里还没有安静下来。"], en: ["Still Here at 3 AM", "The night was deep, but the room was not quiet yet."] },
  ninety_nine_days_passed: { zh: ["九十九天以后", "时间走过九十九天，这个房间仍在等待新的声音。"], en: ["Ninety-Nine Days Later", "Ninety-nine days passed, and the room still waited for another voice."] },
  felix_project: { zh: ["Felix Project", "九十九小时的声音汇聚成了一项漫长工程。"], en: ["Felix Project", "Ninety-nine hours of voices became one long-running project."] },
  the_solitary_island: { zh: ["孤岛", "人群散去后，一位成员独自守着这里九十九分钟。"], en: ["The Solitary Island", "After the crowd left, one person kept watch for ninety-nine minutes."] },
  only_two_on_the_planet: { zh: ["They Were Only Two", "仿佛世界只剩下两个人，而声音仍没有停下。"], en: ["They Were Only Two", "It felt as if only two people remained, and their voices carried on."] },
  the_final_love_story: { zh: ["最后的故事", "同一对伙伴共同走过了九十九次会话。"], en: ["The Final Story", "The same two companions shared ninety-nine sessions."] },
  ninety_nine_active_days: { zh: ["九十九个有声音的日子", "这里在九十九个不同的日子里留下了声音。"], en: ["Ninety-Nine Days of Voices", "Voices were left here on ninety-nine different days."] },
  the_quiet_world: { zh: ["寂静世界再次醒来", "九十九天的安静结束了，熟悉的声音重新抵达。"], en: ["The Quiet World Awoke", "Ninety-nine quiet days ended when a familiar voice returned."] },
  the_last_guard: { zh: ["最后的守望者", "所有人离开后，还有一位成员守了两个小时。"], en: ["The Last Guardian", "After everyone left, one person kept watch for two hours."] },
  little_explorer: { zh: ["Little Explorer", "一位成员完成了十次共同旅程。"], en: ["Little Explorer", "One member completed ten journeys with the room."] },
  under_the_red_star: { zh: ["红星之下", "在房间走过九十九天后的夜晚，又一道声音亮了起来。"], en: ["Under the Red Star", "A voice lit up the night after the room's ninety-ninth day."] },
};

function durationText(seconds: number, language: CoreSnapshot["language"]) { const hours = Math.floor(seconds / 3600); const minutes = Math.floor((seconds % 3600) / 60); return language === "english" ? `${hours}h ${minutes}m` : `${hours} 小时 ${minutes} 分钟`; }
function archiveEventCopy(event: ArchiveEvent, language: CoreSnapshot["language"]): [string, string] {
  const locale = language === "english" ? "en-US" : "zh-CN";
  const dynamicTitle = event.titles?.[locale] || event.titles?.["zh-CN"] || Object.values(event.titles || {})[0];
  const dynamicDescription = event.descriptions?.[locale] || event.descriptions?.["zh-CN"] || Object.values(event.descriptions || {})[0];
  if (dynamicTitle && dynamicDescription) return [dynamicTitle, dynamicDescription];
  const direct = archiveCopy[event.eventType]; if (direct) return language === "english" ? direct.en : direct.zh;
  const voice = event.eventType.match(/^voice_total_(\d+)([hm])$/); if (voice) { const unit = voice[2] === "h" ? "小时" : "分钟"; return language === "english" ? [`${voice[1]}${voice[2]}`, `${voice[1]}${voice[2]} of active voices have been left here.`] : [`${voice[1]} ${unit}`, `这里已经留下了 ${voice[1]} ${unit}有效声音。`]; }
  const age = event.eventType.match(/^room_age_(\d+)d$/); if (age) return language === "english" ? [`${age[1]} Days`, "Time kept moving through this room."] : [`${age[1]} 天`, "时间静静经过了这个房间。"];
  const title = event.eventType.split("_").map((part) => part[0]?.toUpperCase() + part.slice(1)).join(" "); return [title, language === "english" ? "A trace was left in this room." : "这个房间留下了一段新的痕迹。"];
}

function archiveProfile(clientId: string, profiles: Record<string, ArchiveParticipantProfile>, language: CoreSnapshot["language"]): ArchiveParticipantProfile {
  return profiles[clientId] ?? { clientId, displayName: language === "english" ? "Unknown member" : "一位成员", avatar: "?" };
}

function ArchivePersonList({ label, ids, profiles, language }: { label: string; ids: string[]; profiles: Record<string, ArchiveParticipantProfile>; language: CoreSnapshot["language"] }) {
  const [expanded, setExpanded] = useState(false);
  const unique = [...new Set(ids)].filter(Boolean);
  const visible = expanded ? unique : unique.slice(0, 5);
  if (unique.length === 0) return null;
  return <div className="archive-people-row"><span className="archive-people-label">{label}</span><div className="archive-people-list">{visible.map((clientId) => { const profile = archiveProfile(clientId, profiles, language); return <span className="archive-person" key={clientId}><Avatar avatar={profile.avatar} name={profile.displayName} size={20} /><span>{profile.displayName}</span></span>; })}{unique.length > 5 && <button type="button" aria-expanded={expanded} onClick={() => setExpanded(!expanded)}>{expanded ? (language === "english" ? "Collapse" : "收起") : (language === "english" ? `and ${unique.length - 5} more` : `等 ${unique.length - 5} 人`)}</button>}</div></div>;
}

function ArchivePeople({ event, profiles, language }: { event: ArchiveEvent; profiles: Record<string, ArchiveParticipantProfile>; language: CoreSnapshot["language"] }) {
  const triggered = event.triggeredBy.length > 0 ? event.triggeredBy : event.participants;
  const hasAdditionalParticipants = event.triggeredBy.length > 0 && event.participants.some((clientId) => !event.triggeredBy.includes(clientId));
  return <div className="archive-people"><ArchivePersonList label={language === "english" ? "Triggered by" : "触发者"} ids={triggered} profiles={profiles} language={language} />{hasAdditionalParticipants && <ArchivePersonList label={language === "english" ? "Present" : "当时在场"} ids={event.participants} profiles={profiles} language={language} />}</div>;
}

function ArchiveOverlay({ snapshot, channel, all, setAll, close }: { snapshot: CoreSnapshot; channel: Channel; all: boolean; setAll: (value: boolean) => void; close: () => void }) {
  const [archive, setArchive] = useState<RoomArchive | null>(null);
  const [recap,setRecap]=useState(false);
  const [recapEvent,setRecapEvent]=useState('');
  const [loadError, setLoadError] = useState("");
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    setArchive(null);
    setLoadError("");
    if (snapshot.connection !== "connected" || !snapshot.archiveSupported) {
      setLoadError("当前服务器未连接或未开启 Eden Archive。");
      return;
    }
    void (async () => {
      try {
        const stop = await listenForRoomArchive((value) => {
          if (!cancelled && value.stats.roomId === channel.id) setArchive(value);
        });
        if (cancelled) { stop(); return; }
        unlisten = stop;
        // Subscribe first so a fast response cannot arrive before the listener.
        await dispatch({ type: "request_room_archive", channelId: channel.id, all });
      } catch (error) {
        if (!cancelled) setLoadError(String(error));
      }
    })();
    return () => {
      cancelled = true;
      unlisten?.();
      void dispatch({ type: "close_room_archive" }).catch(console.error);
    };
  }, [channel.id, all, snapshot.connection, snapshot.archiveSupported]);
  if (!archive) return <div className="archive-backdrop"><section className="archive-page"><header><div><small>EDEN ARCHIVE</small><h2>{channel.name}</h2></div><button onClick={close}><X size={19} /></button></header><div className="archive-loading">{loadError || snapshot.error || <><span className="spinner" />正在翻阅这个房间留下的痕迹</>}</div></section></div>;
  if (!archive.enabled) return <div className="archive-backdrop" onMouseDown={close}><section className="archive-page" onMouseDown={(event) => event.stopPropagation()}><header><div><small>EDEN ARCHIVE</small><h2>{channel.name}</h2></div><button onClick={close}><X size={19} /></button></header><div className="archive-empty">此服务器没有开启 Eden Archive。</div></section></div>;
  const language = snapshot.language;
  return <div className="archive-backdrop" onMouseDown={close}>
    <ArchiveJournal archive={archive} english={language==='english'} copy={event=>archiveEventCopy(event,language)} close={close} all={all} onAll={()=>setAll(!all)} onOverview={()=>{setRecapEvent('');setRecap(true);}} onRecap={event=>{setRecapEvent(event.id);setRecap(true);}}/>
    {recap&&<ArchiveRecap initialEventId={recapEvent} archive={archive} copy={event=>archiveEventCopy(event,language)} close={()=>setRecap(false)}/>}
  </div>;
}

function ChannelMenu({ x, y, channel, close, isCurrent, canNetwork, canCreate, canEdit, canDelete, showArchive, onArchive, onLeave, onCreateChild, onEdit, onDelete }: { x: number; y: number; channel: Channel; close: () => void; isCurrent: boolean; canNetwork: boolean; canCreate: boolean; canEdit: boolean; canDelete: boolean; showArchive: boolean; onArchive: () => void; onLeave: () => void; onCreateChild: () => void; onEdit: () => void; onDelete: () => void }) {
  const setPolicy = (policy: string) => { dispatch({ type: "set_channel_network_policy", channelId: channel.id, policy }); close(); };
  return <div className="context-menu" style={{ left: x, top: y }}><small>{channel.name}</small>{showArchive && <button onClick={onArchive}><span>查看 Eden Archive</span><Activity size={13} /></button>}{isCurrent && <><button className="danger-text" onClick={onLeave}><span>离开频道</span><LogOut size={13} /></button>{(canNetwork || canCreate || canEdit || canDelete) && <hr />}</>}{canNetwork && <><button onClick={() => setPolicy("auto")}><span>自动选择网络</span>{channel.networkPolicy === "auto" && "✓"}</button><button onClick={() => setPolicy("force_p2p")}><span>优先 P2P</span>{channel.networkPolicy === "force_p2p" && "✓"}</button><button onClick={() => setPolicy("force_sfu")}><span>强制 SFU</span>{channel.networkPolicy === "force_sfu" && "✓"}</button></>}{canNetwork && (canCreate || canEdit || canDelete) && <hr />}{canCreate && <button onClick={onCreateChild}>创建子频道</button>}{canEdit && <button onClick={onEdit}>编辑频道</button>}{canDelete && <button className="danger-text" onClick={onDelete}>删除频道</button>}</div>;
}

function UserMenu({ x, y, user, snapshot, close, onBan, onMove }: { x: number; y: number; user: User; snapshot: CoreSnapshot; close: () => void; onBan: () => void; onMove: () => void }) {
  const [copied, setCopied] = useState(false);
  const [localVolume, setLocalVolume] = useState(user.volume);
  const [editingVolume, setEditingVolume] = useState(false);
  useEffect(() => { if (!editingVolume) setLocalVolume(user.volume); }, [user.volume, editingVolume]);
  const self = user.sessionId === snapshot.selfSessionId;
  const protectedTarget = user.serverRole === "MASTER";
  const channelName = snapshot.channels.find((channel) => channel.id === user.channelId)?.name;
  const roleLabel = user.serverRole === "MASTER" ? "服务器主" : user.serverRole === "ADMIN" ? "服务器管理员" : user.channelRole === "CHANNEL_ADMIN" ? "房间管理员" : "服务器成员";
  const command = (value: Parameters<typeof dispatch>[0]) => { dispatch(value); close(); };
  const copyClientId = async () => {
    if (!user.clientId || !await copyText(user.clientId)) return;
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1400);
  };
  return <div className="context-menu user-menu" style={{ left: x, top: y }} onClick={(event) => event.stopPropagation()}>
    <div className="user-menu-heading"><Avatar avatar={user.avatar} name={user.nickname} size={34} /><div><strong>{user.nickname}</strong><span>{self ? `你自己 · ${roleLabel}` : roleLabel}{channelName ? ` · ${channelName}` : ""}</span><code title={user.clientId}>{user.clientId || "无 Client ID"}</code></div></div>
    <button className="user-copy-id" disabled={!user.clientId} onClick={copyClientId}><span>{copied ? "Client ID 已复制" : "复制 Client ID"}</span>{copied ? <Check size={13} /> : <Copy size={13} />}</button>
    {self && user.channelId && <><hr /><button className="danger-text" onClick={() => command({ type: "leave_channel" })}><span>离开当前频道</span><LogOut size={13} /></button></>}
    {!self && <><hr /><label className="user-volume" title="按服务器和成员身份自动记忆，重连后恢复"><span><span><Volume2 size={12} />本地音量</span><em>{Math.round(localVolume * 100)}%</em></span><input type="range" min="0" max="3" step="0.01" value={localVolume} onPointerDown={() => setEditingVolume(true)} onPointerUp={() => setEditingVolume(false)} onPointerCancel={() => setEditingVolume(false)} onBlur={() => setEditingVolume(false)} onChange={(event) => { const volume = Number(event.target.value); setLocalVolume(volume); dispatch({ type: "set_user_volume", sessionId: user.sessionId, volume }); }} /></label><button onClick={() => command({ type: "set_user_local_mute", sessionId: user.sessionId, muted: !user.locallyMuted })}><span>{user.locallyMuted ? "取消本地静音" : "本地静音"}</span>{user.locallyMuted ? <Check size={13} /> : <VolumeX size={13} />}</button></>}
    {!self && !protectedTarget && snapshot.capabilities.includes("member.move") && <><hr /><button onClick={onMove}><span>移动到其他频道</span><ArrowRightLeft size={13} /></button></>}
    {!self && !protectedTarget && snapshot.capabilities.includes("member.mute") && <button onClick={() => command({ type: "admin_mute", sessionId: user.sessionId, enabled: !user.adminMuted })}><span>{user.adminMuted ? "解除管理员禁言" : "管理员禁言"}</span><MicOff size={13} /></button>}
    {!self && !protectedTarget && snapshot.capabilities.includes("channel.role.set") && <button onClick={() => command({ type: "set_channel_admin", sessionId: user.sessionId, enabled: user.channelRole !== "CHANNEL_ADMIN" })}><span>{user.channelRole === "CHANNEL_ADMIN" ? "移除房间管理员" : "设为房间管理员"}</span><Shield size={13} /></button>}
    {!self && !protectedTarget && snapshot.capabilities.includes("server.role.set") && <button onClick={() => command({ type: "set_server_admin", sessionId: user.sessionId, enabled: user.serverRole !== "ADMIN" })}><span>{user.serverRole === "ADMIN" ? "移除服务器管理员" : "设为服务器管理员"}</span><ShieldCheck size={13} /></button>}
    {!self && !protectedTarget && (snapshot.capabilities.includes("member.kick_channel") || snapshot.capabilities.includes("member.kick_server") || snapshot.capabilities.includes("member.ban")) && <hr />}
    {!self && !protectedTarget && snapshot.capabilities.includes("member.kick_channel") && <button className="danger-text" onClick={() => command({ type: "kick_channel", sessionId: user.sessionId })}><span>踢出当前频道</span><LogOut size={13} /></button>}
    {!self && !protectedTarget && snapshot.capabilities.includes("member.kick_server") && <button className="danger-text" onClick={() => command({ type: "kick_server", sessionId: user.sessionId })}><span>踢出服务器</span><LogOut size={13} /></button>}
    {!self && !protectedTarget && snapshot.capabilities.includes("member.ban") && <button className="danger-text" onClick={onBan}>封禁用户</button>}
  </div>;
}

function BanUserDialog({ user, close }: { user: User; close: () => void }) {
  const [reason, setReason] = useState("");
  return <div className="modal-backdrop compact-backdrop" onMouseDown={close}><form className="channel-editor" onSubmit={(event) => { event.preventDefault(); dispatch({ type: "ban_user", sessionId: user.sessionId, reason: reason.trim() }); close(); }} onMouseDown={(event) => event.stopPropagation()}><header><div><h2>封禁 {user.nickname}</h2><p>封禁由服务端持久化，可在服务器管理中解除。</p></div><button type="button" onClick={close}><X size={18} /></button></header><div className="channel-editor-fields ban-fields"><label>原因<input autoFocus value={reason} maxLength={256} onChange={(event) => setReason(event.target.value)} placeholder="可选" /></label></div><footer><button type="button" onClick={close}>取消</button><button className="delete-button" type="submit">确认封禁</button></footer></form></div>;
}

function MoveUserDialog({ user, channels, close }: { user: User; channels: Channel[]; close: () => void }) {
  const [channelId, setChannelId] = useState(channels.find((channel) => channel.id !== user.channelId)?.id ?? "");
  return <div className="modal-backdrop compact-backdrop" onMouseDown={close}><form className="channel-editor" onSubmit={(event) => { event.preventDefault(); if (channelId) dispatch({ type: "move_user", sessionId: user.sessionId, channelId }); close(); }} onMouseDown={(event) => event.stopPropagation()}><header><div><h2>移动 {user.nickname}</h2><p>选择目标频道，服务端会立即同步给在线成员。</p></div><button type="button" onClick={close}><X size={18} /></button></header><div className="channel-editor-fields ban-fields"><label>目标频道<select autoFocus value={channelId} onChange={(event) => setChannelId(event.target.value)}>{channels.filter((channel) => channel.id !== user.channelId).map((channel) => <option key={channel.id} value={channel.id}>{channel.parentId === "0" ? channel.name : `↳ ${channel.name}`}</option>)}</select></label></div><footer><button type="button" onClick={close}>取消</button><button className="primary" type="submit" disabled={!channelId}>确认移动</button></footer></form></div>;
}

function BanManager({ snapshot, close }: { snapshot: CoreSnapshot; close: () => void }) {
  const page = Math.floor(snapshot.banListOffset / 8);
  useEffect(() => { dispatch({ type: "list_bans", offset: snapshot.banListOffset, limit: 8 }); }, []);
  const date = (value: string) => value ? new Date(value).toLocaleString([], { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit" }) : "";
  return <div className="modal-backdrop compact-backdrop" onMouseDown={close}><section className="ban-manager" onMouseDown={(event) => event.stopPropagation()}><header><div><h2>封禁列表</h2><p>共 {snapshot.banListTotal} 条服务端封禁记录</p></div><button onClick={close}><X size={18} /></button></header><div className="ban-list">{snapshot.bans.length === 0 ? <div className="empty-ban">当前没有封禁记录</div> : snapshot.bans.map((ban: BanRecord) => <article key={ban.clientId}><div><strong>{ban.displayName || ban.clientId}</strong><span>{ban.reason || "未填写原因"}</span><small>{date(ban.createdAt)}{ban.expiresAt ? ` · 到期 ${date(ban.expiresAt)}` : " · 永久"}</small></div><button onClick={() => { dispatch({ type: "unban", clientId: ban.clientId }); window.setTimeout(() => dispatch({ type: "list_bans", offset: snapshot.banListOffset, limit: 8 }), 180); }}>解除封禁</button></article>)}</div><footer><span>第 {page + 1} 页</span><button disabled={snapshot.banListOffset === 0} onClick={() => dispatch({ type: "list_bans", offset: Math.max(0, snapshot.banListOffset - 8), limit: 8 })}>上一页</button><button disabled={snapshot.banListOffset + snapshot.bans.length >= snapshot.banListTotal} onClick={() => dispatch({ type: "list_bans", offset: snapshot.banListOffset + 8, limit: 8 })}>下一页</button></footer></section></div>;
}

function ServerMenu({ x, y, snapshot, close, onMaster, onBans, canViewBans }: { x: number; y: number; snapshot: CoreSnapshot; close: () => void; onMaster: () => void; onBans: () => void; canViewBans: boolean }) {
  const [inviteStatus,setInviteStatus]=useState("");
  const copyInvite=async()=>{try{await navigator.clipboard.writeText(createInvitation(snapshot.serverAddress));setInviteStatus("已复制邀请链接（不含密码）");}catch{setInviteStatus("复制失败，请检查剪贴板权限");}};
  const currentSaved = snapshot.savedServers.some((profile) => profile.address === snapshot.serverAddress);
  const saveCurrentServer = () => {
    if (!snapshot.serverAddress || currentSaved) return;
    const currentNickname = snapshot.users.find((user) => user.sessionId === snapshot.selfSessionId)?.nickname || snapshot.nickname;
    dispatch({ type: "add_saved_server", profile: { displayName: "", serverName: snapshot.serverName, address: snapshot.serverAddress, nickname: currentNickname } });
    close();
  };
  const goToConnectionPage = () => { dispatch({ type: "disconnect" }); close(); };
  const switchServer = async (profile: SavedServer) => {
    if (profile.address === snapshot.serverAddress) return close();
    await dispatch({ type: "disconnect" });
    await dispatch({ type: "connect", address: profile.address, nickname: profile.nickname || snapshot.nickname, remember: true });
    close();
  };
  return <div className="context-menu server-menu" style={{ left: x, top: y }} onClick={(event) => event.stopPropagation()}><small>当前服务器</small><button onClick={copyInvite}>复制服务器邀请链接</button>{inviteStatus&&<small role="status">{inviteStatus}</small>}<button className="save-current-server" disabled={currentSaved || !snapshot.serverAddress} onClick={saveCurrentServer}><span>{currentSaved ? "当前服务器已收藏" : "收藏当前服务器"}</span>{currentSaved ? <Check size={13} /> : <Plus size={13} />}</button><div className="switch-server-group"><button className="switch-server" aria-haspopup="menu"><span><ArrowRightLeft size={14} />切换服务器</span><ChevronRight size={13} /></button><div className="server-switch-submenu" role="menu"><small>切换服务器</small><button className="connection-page-link" onClick={goToConnectionPage}><span><ArrowRightLeft size={14} />前往连接页</span><ChevronRight size={13} /></button><hr /><small>已保存服务器</small><div className="saved-server-list">{snapshot.savedServers.length === 0 ? <p>暂无保存记录</p> : snapshot.savedServers.map((profile, index) => { const current = profile.address === snapshot.serverAddress; return <button className={`saved-server-item ${current ? "current" : ""}`} key={`${profile.address}-${index}`} onClick={() => switchServer(profile)}><span><strong>{profile.displayName || profile.serverName || profile.address}</strong><small>{profile.address}</small></span>{current && <Check size={13} />}</button>; })}</div></div></div><hr /><button onClick={onMaster}>输入 Master Token</button>{canViewBans && <button onClick={onBans}>管理封禁列表</button>}<hr /><button className="danger-text" onClick={goToConnectionPage}>断开连接</button></div>;
}

function MasterTokenDialog({ close }: { close: () => void }) {
  const [token, setToken] = useState("");
  return <div className="modal-backdrop compact-backdrop" onMouseDown={close}><form className="channel-editor" onSubmit={(event) => { event.preventDefault(); if (token.trim()) dispatch({ type: "provision_master", token: token.trim() }); close(); }} onMouseDown={(event) => event.stopPropagation()}><header><div><h2>设置服务器主</h2><p>Token 仅发送给当前服务器验证，不会显示在成员列表中。</p></div><button type="button" onClick={close}><X size={18} /></button></header><div className="channel-editor-fields ban-fields"><label>Master Token<input autoFocus type="password" value={token} onChange={(event) => setToken(event.target.value)} placeholder="输入服务器生成的 Token" /></label></div><footer><button type="button" onClick={close}>取消</button><button className="primary" type="submit" disabled={!token.trim()}>验证 Token</button></footer></form></div>;
}

function miniOverlayRenderKey(snapshot: CoreSnapshot) {
  const channelId = snapshot.users.find((user) => user.sessionId === snapshot.selfSessionId)?.channelId;
  return JSON.stringify([
    snapshot.theme,
    snapshot.miniOverlayMode,
    snapshot.selfSessionId,
    snapshot.avatarCacheRevision,
    snapshot.users.filter((user) => Boolean(channelId) && user.channelId === channelId).map((user) => [user.sessionId, user.nickname, user.avatar, user.channelId, user.muted, user.adminMuted, user.deafened, user.speaking]),
  ]);
}

function MiniOverlay({ snapshot }: { snapshot: CoreSnapshot }) {
  const dragging = useRef(false);
  const self = snapshot.users.find((user) => user.sessionId === snapshot.selfSessionId);
  const members = snapshot.users
    .filter((user) => Boolean(self?.channelId) && user.channelId === self?.channelId)
    .sort((left, right) => Number(right.speaking) - Number(left.speaking) || left.nickname.localeCompare(right.nickname, "zh-CN"));
  const previewMode = new URLSearchParams(window.location.search).get("mini");
  const mode = previewMode === "avatars" || previewMode === "list" ? previewMode : snapshot.miniOverlayMode;
  const visibleMembers = members.slice(0, mode === "avatars" ? 6 : 8);
  const beginDragging = (event: React.PointerEvent<HTMLElement>) => {
    if (event.button !== 0 || dragging.current) return;
    event.preventDefault();
    dragging.current = true;
    void startWindowDragging().catch(() => undefined).finally(() => { dragging.current = false; });
  };
  return <main className={`mini-overlay theme-${snapshot.theme} mode-${mode}`} aria-label="EdenVoice 迷你悬浮窗" onPointerDown={beginDragging} onDragStart={(event) => event.preventDefault()}>
    {visibleMembers.length === 0 ? <div className="mini-empty"><Users size={16} /><span>尚未加入语音频道</span></div> : mode === "avatars" ? <div className="mini-avatar-strip">
      {visibleMembers.map((user) => <div className={`mini-avatar-member ${user.speaking ? "speaking" : ""}`} key={user.sessionId} title={user.nickname}>
        <Avatar avatar={user.avatar} name={user.nickname} size={30} speaking={user.speaking} previewable={false} />
        {(user.muted || user.adminMuted || user.deafened) && <span className="mini-avatar-status">{user.deafened ? <HeadphoneOff size={10} /> : <MicOff size={10} />}</span>}
      </div>)}
      {members.length > visibleMembers.length && <span className="mini-more">+{members.length - visibleMembers.length}</span>}
    </div> : <div className="mini-member-list">
      {visibleMembers.map((user) => <div className={`mini-list-member ${user.speaking ? "speaking" : ""}`} key={user.sessionId}>
        <Avatar avatar={user.avatar} name={user.nickname} size={24} speaking={user.speaking} previewable={false} />
        <span className="mini-member-name">{user.nickname}</span>
        <span className="mini-member-status">{user.deafened ? <HeadphoneOff size={12} /> : user.muted || user.adminMuted ? <MicOff size={12} /> : user.speaking ? <Signal size={12} /> : null}</span>
      </div>)}
    </div>}
  </main>;
}

export function App() {
  const [snapshot, setSnapshot] = useState<CoreSnapshot | null>(null);
  const [bootAttempt, setBootAttempt] = useState(0);
  const [bootError, setBootError] = useState("");
  const [commandError, setCommandError] = useState("");
  const [dismissedError, setDismissedError] = useState("");
  const roomSocial = useRoomSocial(MINI_WINDOW ? null : snapshot, snapshot?.users.find(user => user.sessionId === snapshot.selfSessionId)?.channelId || "");
  const [compactMode, setCompactMode] = useState(false);
  const [settingsPage, setSettingsPage] = useState<SettingsPage | null>(null);
  const [membersVisible, setMembersVisible] = useState(false);
  const membersVisibleRef = useRef(false);
  membersVisibleRef.current=membersVisible;
  const memberResizeBusy = useRef(false);
  const memberExpanded = useRef(false);
  const [memberOverlay,setMemberOverlay] = useState(false);
  const [sidebarWidth,setSidebarWidth] = useState(232);
  const toggleMembers = async () => {
    if(memberResizeBusy.current)return;
    memberResizeBusy.current=true;
    try {
      if(!membersVisibleRef.current){
        setSidebarWidth(document.querySelector('.workspace')?.firstElementChild?.getBoundingClientRect().width || 232);
        const expanded=await resizeMemberPanel(true).catch(()=>false);
        memberExpanded.current=expanded;setMemberOverlay(!expanded);setMembersVisible(true);
      } else {
        if(memberExpanded.current)await resizeMemberPanel(false).catch(()=>false);
        memberExpanded.current=false;setMembersVisible(false);setMemberOverlay(false);
      }
    } finally {memberResizeBusy.current=false;}
  };
  const pushToTalkHeld = useRef(false);
  const previousNotificationState = useRef<RoomNotificationState | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number; channel: Channel } | null>(null);
  const [userMenu, setUserMenu] = useState<{ x: number; y: number; user: User } | null>(null);
  const [serverMenu, setServerMenu] = useState<{ x: number; y: number } | null>(null);
  const [mainMenu, setMainMenu] = useState<{ x: number; y: number } | null>(null);
  const [networkDetails, setNetworkDetails] = useState(false);
  const [quickJump,setQuickJump]=useState(false);
  const [masterToken, setMasterToken] = useState(false);
  const [banUser, setBanUser] = useState<User | null>(null);
  const [moveUser, setMoveUser] = useState<User | null>(null);
  const [banManager, setBanManager] = useState(false);
  const [channelEditor, setChannelEditor] = useState<{ mode: "top" | "child" | "edit"; channel?: Channel } | null>(null);
  const [deleteChannel, setDeleteChannel] = useState<Channel | null>(null);
  const [archiveChannel, setArchiveChannel] = useState<Channel | null>(null);
  const [archiveAll, setArchiveAll] = useState(false);
  const changeWindowMode = async (compact: boolean) => {
    if(memberResizeBusy.current)return;
    if(membersVisibleRef.current)await toggleMembers();
    setCompactMode(compact);
    setMenu(null);
    setUserMenu(null);
    setServerMenu(null);
    setMainMenu(null);
    setSettingsPage(null);
    setNetworkDetails(false);
    windowAction(compact ? "compact" : "restore");
  };
  useEffect(() => {
    let unlisten: () => void = () => undefined;
    const applySnapshot = (next: CoreSnapshot) => setSnapshot((current) => MINI_WINDOW && current && miniOverlayRenderKey(current) === miniOverlayRenderKey(next) ? current : next);
    let disposed=false;
    setBootError("");
    listenForSnapshots(applySnapshot).then((fn) => { if(disposed)fn();else unlisten = fn; }).catch(error => {
      if (!disposed) setBootError(`无法启动客户端：${String(error)}`);
    });
    return () => {disposed=true;unlisten();};
  }, [bootAttempt]);
  useEffect(() => {
    const failed = (event: Event) => { setCommandError((event as CustomEvent<string>).detail); setDismissedError(""); };
    window.addEventListener("eden-command-error", failed);
    return () => window.removeEventListener("eden-command-error", failed);
  }, []);
  useEffect(()=>{const open=()=>setSettingsPage('audio');window.addEventListener('eden-open-audio-settings',open);return()=>window.removeEventListener('eden-open-audio-settings',open);},[]);
  useEffect(()=>{const open=()=>setQuickJump(true);window.addEventListener('eden-quick-jump',open);return()=>window.removeEventListener('eden-quick-jump',open);},[]);
  useEffect(() => { setDismissedError(""); }, [snapshot?.error]);
  useEffect(() => {
    if (!snapshot || MINI_WINDOW) return;
    const next = roomNotificationState(snapshot);
    const previous = previousNotificationState.current;
    previousNotificationState.current = next;
    const prefs=preferences();
    const quiet=roomPreferences(roomKey(snapshot,next.channelId))?.muted;
    const sounds = !prefs.dnd && prefs.sounds && !quiet ? roomNotificationSounds(previous, next) : [];
    sounds.forEach((sound, index) => window.setTimeout(() => { void playUiSound(sound, snapshot.audio.outputVolume); }, index * 280));
  }, [snapshot?.revision]);
  useEffect(() => {
    if (snapshot && compactMode && snapshot.connection !== "connected" && snapshot.connection !== "reconnecting") changeWindowMode(false);
  }, [snapshot?.connection, compactMode, quickJump]);
  useEffect(() => { const close = () => { setMenu(null); setUserMenu(null); setServerMenu(null); setMainMenu(null); }; window.addEventListener("click", close); return () => window.removeEventListener("click", close); }, []);
  useEffect(() => {
    if (!snapshot) return;
    const editable = (target: EventTarget | null) => target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement;
    const release = () => {
      if (pushToTalkHeld.current) { pushToTalkHeld.current = false; dispatch({ type: "set_push_to_talk_pressed", pressed: false }); }
    };
    const keyDown = (event: KeyboardEvent) => {
      if (event.repeat || event.isComposing) return;
      if ((event.ctrlKey||event.metaKey)&&event.code==='KeyK') {event.preventDefault();setQuickJump(true);return;}
      if(quickJump||editable(event.target))return;
      const binding = shortcutFromEvent(event);
      if (!binding) return;
      if (binding === snapshot.shortcuts.toggleWindow && (snapshot.connection === "connected" || snapshot.connection === "reconnecting")) { event.preventDefault(); changeWindowMode(!compactMode); }
      if (binding === snapshot.shortcuts.toggleMic) { event.preventDefault(); dispatch({ type: "set_muted", muted: !snapshot.muted }); }
      if (binding === snapshot.shortcuts.toggleSpeaker) { event.preventDefault(); dispatch({ type: "set_deafened", deafened: !snapshot.deafened }); }
      if (binding === snapshot.shortcuts.toggleMembers) { event.preventDefault(); void toggleMembers(); }
      if (binding === snapshot.shortcuts.pushToTalk && !pushToTalkHeld.current) { event.preventDefault(); pushToTalkHeld.current = true; dispatch({ type: "set_push_to_talk_pressed", pressed: true }); }
    };
    const keyUp = (event: KeyboardEvent) => {
      const parts = snapshot.shortcuts.pushToTalk.split("+");
      const modifier = event.code.replace(/Left$|Right$/, "").replace("Meta", "Super");
      if (parts.includes(event.code) || parts.includes(modifier)) release();
    };
    const hidden = () => { if (document.hidden) release(); };
    window.addEventListener("keydown", keyDown);
    window.addEventListener("keyup", keyUp);
    window.addEventListener("blur", release);
    document.addEventListener("visibilitychange", hidden);
    return () => {
      window.removeEventListener("keydown", keyDown);
      window.removeEventListener("keyup", keyUp);
      window.removeEventListener("blur", release);
      document.removeEventListener("visibilitychange", hidden);
      release();
    };
  }, [snapshot?.shortcuts.toggleWindow, snapshot?.shortcuts.toggleMic, snapshot?.shortcuts.toggleSpeaker, snapshot?.shortcuts.pushToTalk, snapshot?.shortcuts.toggleMembers, snapshot?.muted, snapshot?.deafened, snapshot?.connection, compactMode, quickJump]);
  if (!snapshot) return <>{!MINI_WINDOW && <CloseBehaviorDialog />}<div className="boot-screen">{bootError ? <div className="boot-failure" role="alert"><strong>客户端启动失败</strong><p>{bootError}</p><button onClick={() => setBootAttempt(attempt => attempt + 1)}>重试启动</button></div> : <><span className="spinner" />正在启动 EdenVoice Core</>}</div></>;
  if (MINI_WINDOW) return <AvatarCacheContext.Provider value={snapshot.avatarCacheRevision}><MiniOverlay snapshot={snapshot} /></AvatarCacheContext.Provider>;
  const self = snapshot.users.find((user) => user.sessionId === snapshot.selfSessionId);
  const currentChannelId = self?.channelId || "";
  const currentChannel = snapshot.channels.find((channel) => channel.id === currentChannelId);
  const connected = snapshot.connection === "connected" || snapshot.connection === "reconnecting";
  const operationError = commandError || (connected ? snapshot.error : "");
  const liveUserMenu = userMenu ? { ...userMenu, user: snapshot.users.find((user) => user.sessionId === userMenu.user.sessionId) ?? userMenu.user } : null;
  return <AvatarCacheContext.Provider value={snapshot.avatarCacheRevision}><div className={`app-shell theme-${snapshot.theme} ${compactMode && connected ? "compact-mode" : ""}`}>
    {compactMode && connected ? <CompactTitleBar snapshot={snapshot} expand={() => changeWindowMode(false)} /> : <TitleBar snapshot={snapshot} onCompact={() => changeWindowMode(true)} />}
    {!connected ? <ConnectionScreen snapshot={snapshot} /> : compactMode ? <><CompactView snapshot={snapshot} channel={currentChannel} onUserContext={(event, user) => { event.preventDefault(); event.stopPropagation(); setMenu(null); setUserMenu({ ...contextMenuPoint(event, 250), user }); }} /><VoiceDock snapshot={snapshot} channel={currentChannel} onAudioSettings={() => { changeWindowMode(false); setSettingsPage("audio"); }} /></> : <>
      <div style={{"--member-sidebar-width":`${sidebarWidth}px`} as import("react").CSSProperties} className={`workspace ${membersVisible ? (memberOverlay?"members-overlay":"members-outward") : "members-hidden"}`}><Sidebar snapshot={snapshot} social={roomSocial.social} currentChannel={currentChannelId} onSettings={() => setSettingsPage("audio")} onServerMenu={(event) => { event.stopPropagation(); const anchor = event.currentTarget.getBoundingClientRect(); setServerMenu({ x: anchor.left + 8, y: anchor.bottom + 6 }); }} onCreate={() => setChannelEditor({ mode: "top" })} onChannelContext={(event, channel) => { event.preventDefault(); event.stopPropagation(); setUserMenu(null); setMenu({ ...contextMenuPoint(event, 190), channel }); }} onUserContext={(event, user) => { event.preventDefault(); event.stopPropagation(); setMenu(null); setUserMenu({ ...contextMenuPoint(event, 250), user }); }} /><Chat roomSocial={roomSocial} onPlayerSettings={() => setSettingsPage("player")} snapshot={snapshot} channel={currentChannel} membersVisible={membersVisible} onToggleMembers={() => { void toggleMembers(); setMainMenu(null); }} onMore={(event) => { event.stopPropagation(); const anchor = event.currentTarget.getBoundingClientRect(); setServerMenu(null); setMainMenu((current) => current ? null : { x: Math.max(8, anchor.right - 198), y: anchor.bottom + 7 }); }} />{membersVisible && <MemberPanel snapshot={snapshot} onUserContext={(event, user) => { event.preventDefault(); event.stopPropagation(); setMenu(null); setUserMenu({ ...contextMenuPoint(event, 250), user }); }} />}</div>
      <VoiceDock snapshot={snapshot} channel={currentChannel} onAudioSettings={() => setSettingsPage("audio")} />
    </>}
    <DeviceNotice snapshot={snapshot}/>
    {quickJump&&<QuickJump snapshot={snapshot} close={()=>setQuickJump(false)} onMember={id=>{const user=snapshot.users.find(user=>user.sessionId===id);if(user)setUserMenu({x:Math.max(8,window.innerWidth/2-125),y:100,user});}}/>}
    <MutedSpeechNotice snapshot={snapshot}/><CloseBehaviorDialog theme={snapshot.theme} />
    {operationError && operationError !== dismissedError && <div className="operation-error" role="alert"><CircleAlert size={16}/><span>{operationError}</span><button aria-label="关闭错误提示" onClick={() => { setDismissedError(operationError); setCommandError(""); }}><X size={14}/></button></div>}
    <AnnouncementNotice snapshot={snapshot}/><ServerInvitation snapshot={snapshot}/><UpdateNotice/><PasswordRoom snapshot={snapshot}/>{settingsPage && <SettingsModal snapshot={snapshot} initialPage={settingsPage} close={() => setSettingsPage(null)} />}
    {networkDetails && <NetworkDetails snapshot={snapshot} channel={currentChannel} close={() => setNetworkDetails(false)} />}
    {menu && <ChannelMenu {...menu} showArchive={snapshot.archiveSupported && localStorage.getItem("edenArchiveEnabled") !== "false"} close={() => setMenu(null)} isCurrent={menu.channel.id === currentChannelId} canNetwork={snapshot.capabilities.includes("network.policy.set") && snapshot.networkPolicyChannelIds.includes(menu.channel.id)} canCreate={snapshot.capabilities.includes("channel.create")} canEdit={snapshot.capabilities.includes("channel.edit")} canDelete={snapshot.capabilities.includes("channel.delete")} onArchive={() => { setArchiveChannel(menu.channel); setArchiveAll(false); setMenu(null); }} onLeave={() => { dispatch({ type: "leave_channel" }); setMenu(null); }} onCreateChild={() => { setChannelEditor({ mode: "child", channel: menu.channel }); setMenu(null); }} onEdit={() => { setChannelEditor({ mode: "edit", channel: menu.channel }); setMenu(null); }} onDelete={() => { setDeleteChannel(menu.channel); setMenu(null); }} />}
    {liveUserMenu && <UserMenu {...liveUserMenu} snapshot={snapshot} close={() => setUserMenu(null)} onBan={() => { setBanUser(liveUserMenu.user); setUserMenu(null); }} onMove={() => { setMoveUser(liveUserMenu.user); setUserMenu(null); }} />}
    {serverMenu && <ServerMenu {...serverMenu} snapshot={snapshot} close={() => setServerMenu(null)} onMaster={() => { setMasterToken(true); setServerMenu(null); }} canViewBans={snapshot.capabilities.includes("member.ban_list")} onBans={() => { setBanManager(true); setServerMenu(null); dispatch({ type: "list_bans", offset: 0, limit: 8 }); }} />}
    {mainMenu && <MainMenu {...mainMenu} snapshot={snapshot} close={() => setMainMenu(null)} onCompact={() => changeWindowMode(true)} onNetworkDetails={() => { setNetworkDetails(true); setMainMenu(null); }} onShortcuts={() => { setSettingsPage("shortcuts"); setMainMenu(null); }} onSettings={() => { setSettingsPage("audio"); setMainMenu(null); }} onCreate={() => { setChannelEditor({ mode: "top" }); setMainMenu(null); }} onBans={() => { setBanManager(true); setMainMenu(null); dispatch({ type: "list_bans", offset: 0, limit: 8 }); }} onMaster={() => { setMasterToken(true); setMainMenu(null); }} />}
    {channelEditor && <ChannelEditor passwordSupported={snapshot.passwordRoomsSupported} {...channelEditor} channels={snapshot.channels} close={() => setChannelEditor(null)} />}
    {deleteChannel && <DeleteChannelConfirm channel={deleteChannel} close={() => setDeleteChannel(null)} />}
    {banUser && <BanUserDialog user={banUser} close={() => setBanUser(null)} />}
    {moveUser && <MoveUserDialog user={moveUser} channels={snapshot.channels} close={() => setMoveUser(null)} />}
    {banManager && <BanManager snapshot={snapshot} close={() => setBanManager(false)} />}
    {masterToken && <MasterTokenDialog close={() => setMasterToken(false)} />}
    {archiveChannel && <ArchiveOverlay snapshot={snapshot} channel={archiveChannel} all={archiveAll} setAll={setArchiveAll} close={() => setArchiveChannel(null)} />}
  </div></AvatarCacheContext.Provider>;
}
