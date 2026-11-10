import type { ScreenRequest } from "./screenShareController";
export type ConnectionState = "disconnected" | "connecting" | "connected" | "reconnecting" | "error";

export interface Channel {
  id: string;
  parentId: string;
  name: string;
  sortOrder: number;
  userLimit: number;
  networkPolicy: string;
  audioQualityPolicy: string;
  temporary: boolean;
}

export interface User {
  gameActivity?: import("./room-presentation").GameActivity | null;
  sessionId: string;
  clientId: string;
  nickname: string;
  avatar: string;
  channelId: string;
  muted: boolean;
  deafened: boolean;
  adminMuted: boolean;
  locallyMuted: boolean;
  speaking: boolean;
  serverRole: string;
  channelRole: string;
  volume: number;
}

export interface ChatMessage {
  id: string;
  channelId: string;
  senderSessionId: string;
  senderClientId?: string;
  senderName: string;
  senderAvatar: string;
  text: string;
  sentAtUnixMs: number;
  image?: RoomImageMetadata | null;
}

export interface ArchiveGameMemory { game_id: string; title: string; started_at: string; ended_at: string; duration_seconds: number; }
export interface ArchiveEvent {
  evening?: {started_at:string; ended_at:string; duration_seconds:number; games:string[]; movies:string[]}|null;
  game?: ArchiveGameMemory | null;
  id: string;
  eventType: string;
  category: string;
  occurredAt: string;
  triggeredBy: string[];
  participants: string[];
  importance: number;
  rarity: "common" | "uncommon" | "rare" | "very_rare" | "legendary";
  ruleVersion?: number;
  titles?: Record<string, string>;
  descriptions?: Record<string, string>;
  titleKey: string;
  descriptionKey: string;
}

export interface ArchiveParticipantProfile {
  clientId: string;
  displayName: string;
  avatar: string;
}

export interface RoomArchive {
  stats: {
    roomId: string; roomName: string; createdAt: string; activeVoiceSeconds: number;
    sessionCount: number; longestSessionSeconds: number; maxConcurrentMembers: number;
    activeDays: number; historicalMembers: number;
  };
  ageDays: number;
  events: ArchiveEvent[];
  participantProfiles: Record<string, ArchiveParticipantProfile>;
  hasMore: boolean;
  enabled: boolean;
}

export interface AudioDeviceState {
  inputs: string[];
  outputs: string[];
  selectedInput: string;
  selectedOutput: string;
  inputGain: number;
  outputVolume: number;
  inputMode: string;
  noiseSuppression: string;
  noiseSuppressionStrength: "low" | "medium" | "high";
  echoCancellation: boolean;
  automaticGain: boolean;
  state: string;
  micTestStatus: "idle" | "recording" | "preview_ready" | "playing_original" | "playing_processed";
  deviceError: string;
  dspError: string;
  activeInput: string;
  activeOutput: string;
  inputSampleRate: number | null;
  outputSampleRate: number | null;
}

export interface VoiceDiagnostics {
  rttMs: number | null;
  controlRttMs: number | null;
  jitterMs: number;
  effectiveAudioLossPercent: number;
  transport: string;
  networkQuality: string;
  opusBitrateBps: number;
  fecRecoveredPackets: number;
  plcFrames: number;
  jitterBufferDelayMs: number;
  jitterDepthFrames: number;
  jitterBufferUnderruns: number;
  playbackUnderruns: number;
  captureOverruns: number;
  timing?: {
    captureAgeMs: number; playbackQueueMs: number; actualJitterMs: number;
    playoutLatenessP99Ms: number; sentFecPercent: number;
    workerP95Ms: number; workerP99Ms: number; captureProcessingP95Ms: number;
    captureStaleDrops: number; dspSilenceFrames: number; renderReferenceDrops: number; aecDelayMs: number;
  };
  packetsSent: number;
  packetsReceived: number;
  packetsLost: number;
  packetsReordered: number;
}

export interface BanRecord {
  clientId: string;
  displayName: string;
  reason: string;
  createdAt: string;
  expiresAt: string;
  operatorClientId: string;
}

export interface SavedServer {
  displayName: string;
  serverName: string;
  address: string;
  nickname: string;
}

export interface ShortcutState {
  toggleWindow: string;
  toggleMic: string;
  toggleSpeaker: string;
  pushToTalk: string;
  toggleMembers: string;
}

export interface CoreSnapshot {
  chatQuerySupported?: boolean;
  chatHeads?: Record<string,string>;
  announcementScope?: string;
  announcementRevision?: string;
  announcement?: import("./announcementState").Announcement | null;
  passwordRoomsSupported?: boolean;
  lockedChannelIds?: string[];
  roomScenes?: Record<string, {game_id: string; title: string}>;
  revision: number;
  connection: ConnectionState;
  connectionDetail: string;
  error: string;
  trustEndpoint: string;
  trustFingerprint: string;
  serverName: string;
  serverAddress: string;
  nickname: string;
  avatar: string;
  clientId: string;
  selfSessionId: string;
  serverRole: string;
  theme: "system" | "light" | "dark";
  language: "chinese" | "english";
  shareGameActivity: boolean;
  gameDetectionAvailable: boolean;
  gameActivitySupported: boolean;
  miniOverlayEnabled: boolean;
  miniOverlayMode: "avatars" | "list";
  autoConnectLastServer: boolean;
  autoConnectServer: string;
  autoConnecting: boolean;
  savedServers: SavedServer[];
  capabilities: string[];
  networkPolicyChannelIds: string[];
  bans: BanRecord[];
  banListOffset: number;
  banListTotal: number;
  channels: Channel[];
  users: User[];
  avatarCacheRevision: number;
  messages: ChatMessage[];
  archiveSupported: boolean;
  selfAway?: boolean;
  mutedSpeechNotice?: {sequence: string; atUnixMs: number} | null;
  roomSocialEpoch?: string;
  roomSocialSupported?: boolean;
  roomImagesSupported?: boolean;
  screenShareSupported?: boolean;
  watchSupported: boolean;
  muted: boolean;
  deafened: boolean;
  inputLevel: number;
  outputLevel: number;
  latencyMs: number | null;
  controlLatencyMs: number | null;
  transport: string;
  networkQuality: string;
  diagnostics: VoiceDiagnostics;
  audio: AudioDeviceState;
  shortcuts: ShortcutState;
}

export type CoreCommand =
  | { type: "retry_audio" }
  | { type: "set_share_game_activity"; enabled: boolean }
  | { type: "set_mpv_path"; path: string }
  | { type: "screen_share"; server_address: string; self_session_id: string; request: ScreenRequest }
  | { type: "watch_room"; action: "status" | "open" | "join" | "close" | "transfer_host"; targetSessionId?: string; channelId: string; sessionId: string; serverAddress: string }
  | { type: "watch_open"; file: string; channelId: string; sessionId: string; serverAddress: string }
  | { type: "watch_reopen"; channelId: string; sessionId: string; serverAddress: string }
  | { type: "watch_control"; action: "play" | "pause" | "seek" | "sync"; position?: number }
  | { type: "watch_leave" }
  | { type: "connect"; serverPassword?: string; address: string; nickname: string; remember: boolean; autoConnect?: boolean }
  | { type: "disconnect" }
  | { type: "join_protected_channel"; channelId: string; password: string }
  | { type: "set_channel_password"; channelId: string; password: string }
  | { type: "join_channel"; channelId: string }
  | { type: "leave_channel" }
  | { type: "set_muted"; muted: boolean }
  | { type: "set_deafened"; deafened: boolean }
  | { type: "set_push_to_talk_pressed"; pressed: boolean }
  | { type: "send_text_message"; text: string }
  | { type: "request_room_archive"; channelId: string; all: boolean }
  | { type: "close_room_archive" }
  | { type: "set_user_volume"; sessionId: string; volume: number }
  | { type: "set_user_local_mute"; sessionId: string; muted: boolean }
  | { type: "admin_mute"; sessionId: string; enabled: boolean }
  | { type: "kick_channel"; sessionId: string }
  | { type: "kick_server"; sessionId: string }
  | { type: "ban_user"; sessionId: string; reason: string }
  | { type: "move_user"; sessionId: string; channelId: string }
  | { type: "list_bans"; offset: number; limit: number }
  | { type: "unban"; clientId: string }
  | { type: "set_channel_admin"; sessionId: string; enabled: boolean }
  | { type: "set_server_admin"; sessionId: string; enabled: boolean }
  | { type: "set_channel_network_policy"; channelId: string; policy: string }
  | { type: "create_channel"; password?: string; channel: Channel }
  | { type: "edit_channel"; channel: Channel }
  | { type: "delete_channel"; channelId: string }
  | { type: "set_avatar"; avatar: string }
  | { type: "set_display_name"; displayName: string }
  | { type: "update_saved_server"; index: number; profile: SavedServer }
  | { type: "add_saved_server"; profile: SavedServer }
  | { type: "delete_saved_server"; index: number }
  | { type: "set_theme"; theme: "system" | "light" | "dark" }
  | { type: "set_language"; language: "chinese" | "english" }
  | { type: "set_mini_overlay"; enabled: boolean; mode: "avatars" | "list" }
  | { type: "set_auto_connect_last_server"; enabled: boolean }
  | { type: "set_auto_connect"; enabled: boolean; serverAddress: string }
  | { type: "update_shortcuts"; shortcuts: ShortcutState }
  | {
      type: "update_audio_preferences";
      inputDevice: string;
      outputDevice: string;
      inputGain: number;
      outputVolume: number;
      inputMode: string;
      noiseSuppression: string;
      noiseSuppressionStrength: "low" | "medium" | "high";
      echoCancellation: boolean;
      automaticGain: boolean;
    }
  | { type: "provision_master"; token: string }
  | { type: "trust_server"; endpoint: string; fingerprint: string }
  | { type: "refresh_devices" }
  | { type: "play_output_test" }
  | { type: "start_mic_test" }
  | { type: "play_mic_test_original" }
  | { type: "play_mic_test_processed" }
  | { type: "stop_mic_test" };


export interface RoomImageMetadata {
  reference: string;
  mime: "image/jpeg";
  width: number;
  height: number;
  bytes: number;
}

export interface RoomSocialContext {
  serverAddress: string;
  selfSessionId: string;
  channelId: string;
  serverFingerprint?: string;
}

export interface RoomSocialRequest {
  request_id?: string;
  channel_id: string;
  action: "status" | "away" | "reaction" | "image_begin" | "image_chunk" | "image_commit" | "image_cancel" | "image_get";
  active?: boolean;
  emoji?: string;
  bytes?: number;
  width?: number;
  height?: number;
  upload_id?: string;
  offset?: number;
  data?: string;
  reference?: string;
}

export interface RoomSocialEvent {
  request_id?: string;
  channel_id: string;
  server_address: string;
  self_session_id: string;
  server_fingerprint?: string;
  kind: "error" | "status" | "away" | "reaction" | "image_upload" | "image_committed" | "image_cancelled" | "image_data";
  code?: string;
  message?: string;
  retry_after_ms?: number;
  away?: {session_id: string}[];
  session_id?: string;
  active?: boolean;
  emoji?: string;
  expires_at_unix_ms?: number;
  upload_id?: string;
  offset?: number;
  reference?: string;
  data?: string;
  eof?: boolean;
  image?: RoomImageMetadata | null;
}
