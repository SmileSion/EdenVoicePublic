import { useLayoutEffect, useRef, useState } from "react";

// Keep reading anchored to a message, not a pixel scrollTop: the oldest row
// disappears when the rolling 100-message history receives another message.
export function useChatScroll(context: string, messageIds: string[], query: string, visible = true) {
  const container = useRef<HTMLDivElement>(null);
  const following = useRef(true);
  const anchors = useRef<Array<{ id: string; top: number }>>([]);
  const previous = useRef<{ context: string; query: string; last?: string }>(undefined);
  const [hasNewMessages, setHasNewMessages] = useState(false);
  const idsKey = JSON.stringify(messageIds);

  const remember = () => {
    const node = container.current;
    if (!node || !visible) return;
    const top = node.getBoundingClientRect().top;
    anchors.current = Array.from(node.querySelectorAll<HTMLElement>("[data-message-id]"))
      .filter((row) => row.getBoundingClientRect().bottom > top)
      .map((row) => ({ id: row.dataset.messageId!, top: row.getBoundingClientRect().top - top }));
  };
  const jumpToLatest = () => {
    const node = container.current;
    if (!node || !visible) return;
    node.scrollTop = node.scrollHeight;
    following.current = true;
    setHasNewMessages(false);
    remember();
  };
  const onScroll = () => {
    const node = container.current;
    if (!node || !visible) return;
    following.current = node.scrollHeight - node.clientHeight - node.scrollTop <= 32;
    // Reaching the end of search results is not reading the latest messages.
    if (following.current && !query) setHasNewMessages(false);
    remember();
  };

  useLayoutEffect(() => {
    const node = container.current;
    if (!node || !visible) return;
    const old = previous.current;
    const last = messageIds.at(-1);
    const contextChanged = old?.context !== context;
    const queryChanged = old?.query !== query;
    const appended = !!old && old.last !== last && last !== undefined;
    if (contextChanged || (queryChanged && !query)) {
      jumpToLatest();
    } else if (queryChanged) {
      node.scrollTop = 0;
      following.current = false;
    } else if (following.current && !query) {
      jumpToLatest();
    } else {
      const rows = new Map(Array.from(node.querySelectorAll<HTMLElement>("[data-message-id]"))
        .map((row) => [row.dataset.messageId!, row]));
      const anchor = anchors.current.find((item) => rows.has(item.id));
      if (anchor) {
        node.scrollTop += rows.get(anchor.id)!.getBoundingClientRect().top
          - node.getBoundingClientRect().top - anchor.top;
      } else if (anchors.current.length) {
        // If the entire reading position aged out, show the oldest remaining
        // message rather than jumping to an unrelated pixel offset.
        node.scrollTop = 0;
      }
      if (appended) setHasNewMessages(true);
    }
    previous.current = { context, query, last };
    remember();
  }, [context, idsKey, query, visible]);

  return { container, onScroll, hasNewMessages, jumpToLatest };
}
