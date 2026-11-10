import type { CoreSnapshot } from "./types";
import type { UiSound } from "./uiSounds";

export function roomNotificationState(snapshot: CoreSnapshot) {
  const self = snapshot.users.find((user) => user.sessionId === snapshot.selfSessionId);
  const channelId = self?.channelId && self.channelId !== "0" ? self.channelId : "";
  return {
    connection: snapshot.connection,
    server: snapshot.serverAddress,
    self: snapshot.selfSessionId,
    channelId,
    adminMuted: self?.adminMuted ?? false,
    members: new Set(snapshot.users.filter((user) => channelId && user.channelId === channelId && user.sessionId !== snapshot.selfSessionId).map((user) => user.sessionId)),
  };
}

export type RoomNotificationState = ReturnType<typeof roomNotificationState>;

export function roomNotificationSounds(previous: RoomNotificationState | null, next: RoomNotificationState): UiSound[] {
  // Initial snapshots and reconnects are baselines, not a roomful of arrivals.
  if (!previous || previous.connection !== "connected" || next.connection !== "connected" || previous.server !== next.server || previous.self !== next.self) return [];
  const sounds: UiSound[] = [];
  if (next.channelId !== previous.channelId) {
    sounds.push(next.channelId ? "room-joined" : "room-left");
  } else if (next.channelId) {
    // Coalesce bulk moves into one cue of each kind, and ignore other rooms.
    if ([...next.members].some((id) => !previous.members.has(id))) sounds.push("room-joined");
    if ([...previous.members].some((id) => !next.members.has(id))) sounds.push("room-left");
  }
  if (!previous.adminMuted && next.adminMuted) sounds.push("admin-muted");
  return sounds;
}
