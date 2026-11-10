import type { User } from "./types";

export type RoomAppearance = "lounge" | "game";
// Future Windows activity provider: emit only explicitly shared, sanitized metadata.
// No process paths, command lines, executable scanning, or presence upload here.
export interface GameActivity {
  kind: "game";
  title: string;
  source: "windows" | "manual";
  updatedAt: number;
  expiresAt: number;
}
export function gameActivityLabel(activity: GameActivity | undefined | null, now = Date.now()): string | null {
  if (!activity || activity.kind !== "game" || !["windows", "manual"].includes(activity.source)
    || !Number.isFinite(activity.updatedAt) || !Number.isFinite(activity.expiresAt)
    || activity.updatedAt > now || activity.expiresAt <= now) return null;
  const title = activity.title?.trim();
  return title ? title.slice(0, 80) : null;
}
export function appearanceKey(server: string, channel: string) {
  return `eden.room.appearance:${JSON.stringify([server, channel])}`;
}
export function loadAppearance(server: string, channel: string): RoomAppearance {
  try { return localStorage.getItem(appearanceKey(server, channel)) === "game" ? "game" : "lounge"; }
  catch { return "lounge"; }
}
export function saveAppearance(server: string, channel: string, appearance: RoomAppearance) {
  try { localStorage.setItem(appearanceKey(server, channel), appearance); } catch { /* private storage may be unavailable */ }
}
// Stable order avoids moving seats whenever a voice meter or readiness flag changes.
export function seatPreview(users: User[], self: string, host: string, limit = 5): User[] {
  return [...users].sort((a, b) => {
    const rank = (id: string) => id === self ? 0 : id === host ? 1 : 2;
    return rank(a.sessionId) - rank(b.sessionId) || a.sessionId.localeCompare(b.sessionId);
  }).slice(0, limit);
}

// Native snapshots serialize the unassigned channel ID as "0".
export const hasVoiceChannel = (id?: string | null): boolean => !!id && id !== "0";
