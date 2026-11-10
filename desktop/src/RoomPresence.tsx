import { useEffect, useRef, useState } from "react";
import { Coffee, Smile } from "lucide-react";

export interface RoomSocialView {
  supported: boolean;
  away: Record<string, { sessionId: string }>;
  reactions: Array<{ id: string; sessionId: string; emoji: string; expiresAtUnixMs: number }>;
  busy: boolean;
  error: string;
}

type Away = RoomSocialView["away"][string];
type Reaction = RoomSocialView["reactions"][number];
const reactions = ["👍", "❤️", "😂", "🎉", "👏", "☕"];
const reactionNames = ["赞", "喜欢", "开心", "庆祝", "鼓掌", "喝杯咖啡"];
const englishReactionNames = ["Thumbs up", "Love", "Laugh", "Celebrate", "Applause", "Coffee"];

// Render only live room members, even while a room snapshot is catching up.
export function roomPresence(social: RoomSocialView | undefined, sessionIds: string[], now: number, enabled = true) {
  const away: Record<string, Away> = {};
  const latest = new Map<string, Reaction>();
  if (enabled && social?.supported) {
    const members = new Set(sessionIds);
    for (const value of Object.values(social.away)) {
      if (members.has(value.sessionId)) away[value.sessionId] = value;
    }
    for (const value of social.reactions) {
      if (!members.has(value.sessionId) || value.expiresAtUnixMs <= now || !reactions.includes(value.emoji)) continue;
      if ((latest.get(value.sessionId)?.expiresAtUnixMs ?? 0) <= value.expiresAtUnixMs) latest.set(value.sessionId, value);
    }
  }
  return { away, reactions: [...latest.values()].sort((a, b) => a.expiresAtUnixMs - b.expiresAtUnixMs) };
}

export function usePresenceClock(social?: RoomSocialView) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    const current = Date.now();
    const endings = (social?.reactions ?? []).map(value => value.expiresAtUnixMs).filter(end => end > current);
    if (!endings.length) return;
    const timer = window.setTimeout(() => setNow(Date.now()), Math.max(1, Math.min(1000, ...endings.map(end => end - current))));
    return () => window.clearTimeout(timer);
  }, [social, now]);
  return Math.max(now, Date.now());
}

export function RoomSocialActions({ social, ownAway, english, onAway, onReaction }: {
  social: RoomSocialView; ownAway?: Away; english: boolean;
  onAway?: (active: boolean) => void; onReaction?: (emoji: string) => void;
}) {
  const [menu, setMenu] = useState<"reaction" | null>(null);
  const anchor = useRef<HTMLDivElement>(null);
  const awayButton = useRef<HTMLButtonElement>(null), reactionButton = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!menu) return;
    const close = (event: PointerEvent) => { if (!anchor.current?.contains(event.target as Node)) setMenu(null); };
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape") { setMenu(null); reactionButton.current?.focus(); }
    };
    document.addEventListener("pointerdown", close);
    document.addEventListener("keydown", key);
    return () => { document.removeEventListener("pointerdown", close); document.removeEventListener("keydown", key); };
  }, [menu]);
  return <div className="room-social-actions" ref={anchor}>
    <button ref={awayButton} className={ownAway ? "is-away" : ""} aria-label={ownAway ? english ? "Return and unmute" : "返回并开麦" : english ? "Step away and mute" : "暂离并关闭麦克风"} aria-pressed={!!ownAway} title={ownAway ? english ? "Return and turn on your microphone" : "返回并打开麦克风" : english ? "Keep your seat and mute your microphone" : "保留座位并关闭麦克风，开麦后自动结束暂离"} disabled={social.busy || !onAway} onClick={() => { setMenu(null); onAway?.(!ownAway); }}><Coffee size={14}/><span>{ownAway ? english ? "Return & unmute" : "返回并开麦" : english ? "Away" : "暂离"}</span></button>
    <button ref={reactionButton} aria-label={english ? "Send a reaction" : "发送表情回应"} aria-expanded={menu === "reaction"} title={english ? "Send a quiet reaction" : "轻轻回应一下"} disabled={social.busy || !onReaction} onClick={() => setMenu(menu === "reaction" ? null : "reaction")}><Smile size={14}/><span>{english ? "React" : "回应"}</span></button>
    {menu === "reaction" && <div className="room-social-popover room-reaction-options" role="group" aria-label={english ? "Choose a reaction" : "选择表情回应"}>{reactions.map((emoji, index) => <button key={emoji} disabled={social.busy} aria-label={(english ? englishReactionNames : reactionNames)[index]} title={(english ? englishReactionNames : reactionNames)[index]} onClick={() => { setMenu(null); onReaction?.(emoji); reactionButton.current?.focus(); }}>{emoji}</button>)}</div>}
  </div>;
}

export function RoomAwayNotice({ english }: { english: boolean }) {
  return <div className="room-away-notice"><Coffee size={12}/><span>{english ? "Your seat is here · microphone muted" : "座位为你留着 · 麦克风已关闭"}<small>{english ? "Unmute when you're back" : "开麦即结束暂离"}</small></span></div>;
}

export function RoomAwayBadge({ english }: { english: boolean }) {
  const label = english ? "Away · microphone muted" : "暂离 · 麦克风已关闭";
  return <span className="room-away-badge" title={label} aria-label={label}><Coffee size={10}/><span>{english ? "Away" : "暂离"}</span></span>;
}

export function RoomMemberPresence({ sessionId, nickname, social }: { sessionId: string; nickname: string; social?: RoomSocialView }) {
  if (!social?.supported) return null;
  const reaction = social.reactions.find(value => value.sessionId === sessionId && value.expiresAtUnixMs > Date.now());
  return <>{social.away[sessionId] && <span className="member-away" title={`${nickname} 暂离 · 麦克风已关闭`} aria-label={`${nickname} 暂离`}><Coffee size={11}/><span>暂离</span></span>}{reaction && <span key={reaction.id} className="member-reaction" role="img" aria-label={`${nickname} 回应了 ${reaction.emoji}`} title={`${nickname} 回应了 ${reaction.emoji}`}>{reaction.emoji}</span>}</>;
}

export function SeatReaction({ reaction, nickname, now, english }: { reaction: Reaction; nickname: string; now: number; english: boolean }) {
  const age = useRef(-Math.max(0, 2500 - (reaction.expiresAtUnixMs - now)));
  return <div className="seat-reaction" role="img" aria-label={`${nickname}${english ? " reacted " : " 回应了 "}${reaction.emoji}`} style={{ "--reaction-age": `${age.current}ms` } as import("react").CSSProperties}>{reaction.emoji}</div>;
}

export function SideRoomReactions({ reactions: values, names, now, english }: { reactions: Reaction[]; names: Map<string, string>; now: number; english: boolean }) {
  return <div className="room-side-reactions" aria-label={english ? "Room reactions" : "房间表情回应"}>{values.slice(-3).map(reaction => <SideReaction key={reaction.id} reaction={reaction} nickname={names.get(reaction.sessionId) ?? ""} now={now}/>)}</div>;
}

function SideReaction({ reaction, nickname, now }: { reaction: Reaction; nickname: string; now: number }) {
  const age = useRef(-Math.max(0, 2500 - (reaction.expiresAtUnixMs - now)));
  return <div className="room-side-reaction" style={{ "--reaction-age": `${age.current}ms` } as import("react").CSSProperties}><span title={nickname}>{nickname}</span><span role="img" aria-label={reaction.emoji}>{reaction.emoji}</span></div>;
}
