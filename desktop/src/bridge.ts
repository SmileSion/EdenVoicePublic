import { applySnapshotPatch, type SnapshotPatch } from "./snapshotPatch";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { mockRoomArchive, mockSnapshot } from "./mock";
import type { CoreCommand, CoreSnapshot, RoomArchive, RoomSocialContext, RoomSocialRequest, RoomSocialEvent } from "./types";

let lockedChannels: string[]=[];
const rememberLocks=(s:CoreSnapshot)=>{lockedChannels=s.lockedChannelIds||[];return s;};
const hasTauri = () => "__TAURI_INTERNALS__" in window;

export async function getSnapshot(): Promise<CoreSnapshot> {
  return rememberLocks(hasTauri() ? await invoke<CoreSnapshot>("get_snapshot") : structuredClone(mockSnapshot));
}

export async function dispatch(command: CoreCommand): Promise<void> {
  if(command.type==="join_channel"&&lockedChannels.includes(command.channelId)){window.dispatchEvent(new CustomEvent("eden-password-room",{detail:command.channelId}));return;}
  if (hasTauri()) {
    await invoke("dispatch_core", { command });
  } else {
    console.info("[browser preview] core command", command.type);
  }
}

export async function listenForSnapshots(callback: (snapshot: CoreSnapshot) => void): Promise<UnlistenFn> {
  if (!hasTauri()) {
    callback(structuredClone(mockSnapshot));
    return () => undefined;
  }
  let current: CoreSnapshot | null = null;
  let disposed = false;
  let refreshing = false;
  let again = false;
  const apply = (next: CoreSnapshot) => {
    if (disposed || (current && next.revision < current.revision)) return;
    current = next;
    callback(rememberLocks(next));
  };
  const refresh = async () => {
    if (disposed) return;
    if (refreshing) {
      again = true;
      return;
    }
    refreshing = true;
    try {
      apply(await invoke<CoreSnapshot>("get_snapshot"));
    } finally {
      refreshing = false;
      if (again) {
        again = false;
        void refresh().catch(() => {});
      }
    }
  };
  const stopFull = await listen<CoreSnapshot>("core-state", event => apply(event.payload));
  let stopPatch: UnlistenFn;
  try {
    stopPatch = await listen<SnapshotPatch>("core-state-patch", event => {
      if (disposed) return;
      const next = current ? applySnapshotPatch(current, event.payload) : null;
      if (next) {
        if (next !== current) apply(next);
      } else {
        void refresh().catch(() => {});
      }
    });
  } catch (error) {
    stopFull();
    throw error;
  }
  try {
    await refresh();
  } catch (error) {
    disposed = true;
    stopFull();
    stopPatch();
    throw error;
  }
  return () => {
    disposed = true;
    stopFull();
    stopPatch();
  };
}

export async function listenForRoomArchive(callback: (archive: RoomArchive) => void): Promise<UnlistenFn> {
  if (!hasTauri()) {
    callback(structuredClone(mockRoomArchive));
    return () => undefined;
  }
  return listen<RoomArchive>("room-archive", (event) => callback(event.payload));
}

export async function getAvatarData(reference: string): Promise<string | null> {
  if (!hasTauri() || !reference.startsWith("custom:")) return null;
  return invoke<string | null>("get_avatar_data", { reference });
}

export async function exportUserData(password: string): Promise<string | null> {
  if (!hasTauri()) throw new Error("数据导出仅在桌面客户端中可用");
  return invoke<string | null>("export_user_data", { password });
}

export async function importUserData(password: string): Promise<string | null> {
  if (!hasTauri()) throw new Error("数据导入仅在桌面客户端中可用");
  return invoke<string | null>("import_user_data", { password });
}

export async function restartClient(): Promise<void> {
  if (hasTauri()) await invoke("restart_client");
}

export async function saveDiagnosticReport(report: string): Promise<string> {
  if (hasTauri()) return invoke<string>("export_diagnostics", { report });
  const filename = `edenvoice-diagnostics-${new Date().toISOString().replace(/[:.]/g, "-")}.json`;
  const url = URL.createObjectURL(new Blob([report], { type: "application/json" }));
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = filename;
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  window.setTimeout(() => URL.revokeObjectURL(url), 1000);
  return filename;
}

export async function windowAction(action: "minimize" | "maximize" | "compact" | "restore" | "close"): Promise<void> {
  if (hasTauri()) await invoke("window_action", { action });
}

export async function startWindowDragging(): Promise<void> {
  if (hasTauri()) await getCurrentWindow().startDragging();
}

export async function resizeMemberPanel(open: boolean): Promise<boolean> {
  return hasTauri() ? invoke<boolean>("resize_member_panel", {open}) : false;
}


export class RoomSocialError extends Error {
  constructor(message: string, readonly code = "request_failed", readonly retryAfterMs = 0) {
    super(message);
    this.name = "RoomSocialError";
  }
}

const roomSocialErrors: Record<string, string> = {
  busy: "房间互动有点忙，请稍后再试",
  forbidden: "你已不在这个房间，请重新进入后再试",
  invalid_request: "这次操作未能完成，请重试",
  unavailable: "服务器暂时无法处理图片，请稍后再试",
  secure_mode_required: "图片分享需要启用加密连接，请联系服务器管理员",
  storage_full: "服务器图片空间已满，请联系管理员清理后再试",
  invalid_image: "图片格式或大小不符合要求，请重新选择图片",
  upload_exists: "已有图片正在发送，请等待或先取消",
  upload_missing: "图片发送已过期或取消，请重新发送",
  invalid_chunk: "图片传输进度不一致，请重新发送",
  incomplete_image: "图片尚未传输完整，请重新发送",
  message_failed: "图片未能发送到房间，请检查连接后重试",
  image_missing: "图片已过期或暂时无法读取",
};

/** The native worker correlates each reply with the room and authenticated session. */
export async function requestRoomSocial(context: RoomSocialContext, request: RoomSocialRequest): Promise<RoomSocialEvent> {
  if (!hasTauri()) throw new RoomSocialError("房间互动仅在已连接的桌面客户端中可用", "unsupported");
  const requestId = request.request_id || crypto.randomUUID();
  let event: RoomSocialEvent;
  try {
    event = await invoke<RoomSocialEvent>("request_room_social", {context, request: {...request, request_id: requestId}});
  } catch (error) {
    throw new RoomSocialError(typeof error === "string" ? error : "房间互动请求失败，请检查连接后重试");
  }
  if (event.server_address !== context.serverAddress || event.self_session_id !== context.selfSessionId
      || event.channel_id !== context.channelId || event.request_id !== requestId
      || (context.serverFingerprint && event.server_fingerprint !== context.serverFingerprint)) {
    throw new RoomSocialError("连接或房间已改变，请重试", "context_changed");
  }
  if (event.kind === "error") throw new RoomSocialError(roomSocialErrors[event.code ?? ""] || "房间互动暂时不可用，请稍后再试", event.code, event.retry_after_ms);
  return event;
}

export async function listenForRoomSocial(callback: (event: RoomSocialEvent) => void): Promise<UnlistenFn> {
  if (!hasTauri()) return () => undefined;
  return listen<RoomSocialEvent>("room-social", event => callback(event.payload));
}
