import assert from "node:assert/strict";
import test from "node:test";
import { chatTimestamp, isOwnChatMessage } from "../src/chatPresentation.ts";
import type { ChatMessage } from "../src/types.ts";

test("chat timestamps show year, date and local time across year boundaries", () => {
  assert.equal(chatTimestamp(new Date(2025, 11, 31, 23, 59).getTime()), "2025-12-31 23:59");
  assert.equal(chatTimestamp(new Date(2026, 0, 1, 0, 1).getTime()), "2026-01-01 00:01");
  assert.equal(chatTimestamp(new Date(2024, 1, 29, 9, 5).getTime()), "2024-02-29 09:05");
  assert.equal(chatTimestamp(NaN), "时间未知");
});

test("persisted messages use stable identity, never recycled session IDs", () => {
  const message = { senderSessionId: "10001", senderClientId: "alice" } as ChatMessage;
  assert.equal(isOwnChatMessage(message, "alice"), true);
  assert.equal(isOwnChatMessage(message, "bob"), false);
  assert.equal(isOwnChatMessage({ ...message, senderClientId: undefined }, "bob"), false);
  assert.equal(isOwnChatMessage(message, ""), false);
});
