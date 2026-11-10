import assert from "node:assert/strict";
import test from "node:test";
import { roomNotificationSounds, roomNotificationState, type RoomNotificationState } from "../src/roomNotifications.ts";
import type { CoreSnapshot } from "../src/types.ts";

function state(overrides: Partial<RoomNotificationState> = {}): RoomNotificationState {
  return { connection: "connected", server: "localhost", self: "1", channelId: "10", adminMuted: false, members: new Set(["2"]), ...overrides };
}

test("same-room arrivals and departures each play one cue", () => {
  assert.deepEqual(roomNotificationSounds(state(), state({ members: new Set(["2", "3", "4"]) })), ["room-joined"]);
  assert.deepEqual(roomNotificationSounds(state(), state({ members: new Set() })), ["room-left"]);
  assert.deepEqual(roomNotificationSounds(state(), state({ members: new Set(["3"]) })), ["room-joined", "room-left"]);
});

test("initial state and connection identity changes are silent", () => {
  const next = state({ members: new Set(["2", "3"]) });
  for (const previous of [null, state({ connection: "disconnected" }), state({ server: "another" }), state({ self: "9" })]) {
    assert.deepEqual(roomNotificationSounds(previous, next), []);
  }
  assert.deepEqual(roomNotificationSounds(state(), state({ connection: "disconnected", members: new Set() })), []);
});

test("self room changes do not also play member cues; admin mute is preserved", () => {
  assert.deepEqual(roomNotificationSounds(state(), state({ channelId: "20", members: new Set(["5", "6"]) })), ["room-joined"]);
  assert.deepEqual(roomNotificationSounds(state(), state({ channelId: "", members: new Set() })), ["room-left"]);
  assert.deepEqual(roomNotificationSounds(state(), state({ adminMuted: true })), ["admin-muted"]);
  assert.deepEqual(roomNotificationSounds(state({ adminMuted: true }), state()), []);
  assert.deepEqual(roomNotificationSounds(state(), state()), []);
});

test("snapshot membership excludes self and users in other rooms", () => {
  const snapshot = {
    connection: "connected", serverAddress: "localhost", selfSessionId: "1",
    users: [{ sessionId: "1", channelId: "10" }, { sessionId: "2", channelId: "10" }, { sessionId: "3", channelId: "20" }],
  } as CoreSnapshot;
  const previous = roomNotificationState(snapshot);
  assert.deepEqual(previous.members, new Set(["2"]));
  snapshot.users = snapshot.users.filter((user) => user.sessionId !== "3");
  assert.deepEqual(roomNotificationSounds(previous, roomNotificationState(snapshot)), []);
  snapshot.users[0].channelId = "0";
  const lobby = roomNotificationState(snapshot);
  assert.equal(lobby.channelId, "");
  assert.equal(lobby.members.size, 0);
  assert.deepEqual(roomNotificationSounds(previous, lobby), ["room-left"]);
});
