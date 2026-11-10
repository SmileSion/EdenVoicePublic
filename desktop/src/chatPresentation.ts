import type { ChatMessage } from "./types";

/** Full local date is always shown, including messages from previous years. */
export function chatTimestamp(timestamp: number): string {
  const date = new Date(timestamp);
  if (!Number.isFinite(date.getTime())) return "时间未知";
  const two = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${two(date.getMonth() + 1)}-${two(date.getDate())} ${two(date.getHours())}:${two(date.getMinutes())}`;
}

export function isOwnChatMessage(message: ChatMessage, clientId: string): boolean {
  // Session IDs are reused after a server restart; only stable identity can
  // attribute persisted messages. Legacy anonymous messages remain neutral.
  return Boolean(clientId && message.senderClientId === clientId);
}
