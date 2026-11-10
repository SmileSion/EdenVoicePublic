# EdenVoice protocol V0.5

V0.5 extends the control envelope with room network policy and lightweight receiver QoE feedback while retaining candidate exchange, route coordination, subscriptions, and binary connectivity probes. Audio DSP and VAD/PTT gating remain local client behavior and do not enter the network route manager.

New control messages:

- `set_room_network_policy`: room-level `auto`, `force_sfu`, or `force_p2p`, plus an optional target `channel_id`. A missing/zero target retains the compatible behavior of applying to the sender's current room. Policy is separate from the current transitional transport.
- `audio_qoe_feedback`: 500 ms receiver metrics forwarded only to the corresponding sender in the same room. It carries transport/late/effective loss, burst and reorder data, jitter-buffer/PLC/FEC counters, and resync counts.

`route_update` now includes `room_network_policy`; older clients safely ignore it and V0.4 clients default a missing field to `auto`. Each channel in `server_state` also carries its authoritative `network_policy`, allowing an administrator to edit a selected room without first joining it. A missing channel field remains compatible and means `auto`.

## Control transport

Control uses TCP with UTF-8 NDJSON: one flat JSON object followed by `\n`. A message is at most 64 KiB. Every object has `type`; message fields sit beside it:

```json
{"type":"hello","nickname":"Alice"}
```

The nickname is valid UTF-8 from 1 through 64 bytes. In v0.5 the TLS 1.3 connection begins with `HELLO` carrying protocol version 5, security capabilities, the persistent client identity public key/ClientID, and an ephemeral X25519 public key. The server returns `IDENTITY_CHALLENGE`; the client signs the bound transcript before `WELCOME`. The server closes connections that do not complete this sequence or exceed resource limits. Malformed or unsupported post-handshake messages produce `ERROR`; normal invalid input must not panic either endpoint.

## Control messages

| Type | Direction | Data |
|---|---|---|
| `hello` | C→S | `nickname` |
| `welcome` | S→C | `session_id`, `server_name`, `voice_addr`, bounded `media_queue_size` |
| `server_state` | S→C | `server_name`, `channels`, `users` |
| `join_channel` | C→S | `channel_id` |
| `leave_channel` | C→S | none |
| `channel_joined` | S→C | reserved acknowledgement |
| `user_joined` | S→C | `user` |
| `user_left` | S→C | `session_id` |
| `user_moved` | S→C | `session_id`, `channel_id` (zero means no channel) |
| `set_mute`, `set_deafen` | C→S | `enabled` |
| `user_state_changed` | S→C | `session_id`, effective `muted`, `deafened`, and optional `admin_muted` (missing means false for legacy compatibility) |
| `ping`, `pong` | both | opaque numeric `nonce` |
| `error` | S→C | stable `code`, human-readable `message` |
| `register_candidates` | C→S | host/IPv6 candidates and priority |
| `peer_candidates` | S→C | peer ID, generation, candidates including server-reflexive |
| `network_report` | C→S | peer ID, qualified, RTT, loss, jitter |
| `route_update` | S→C | room topology, selected transport, reason, timing thresholds, and degradation hysteresis thresholds |
| `set_transport_mode` | C→S | `auto`, `force_p2p`, `force_sfu`, or `force_relay` |
| `set_room_network_policy` | C→S | `policy` and optional target `channel_id`; server authorization is evaluated against that channel |
| `subscribe_streams` | C→S | compact publisher stream/session IDs |
| `provision_master` | C→S | one-time operator token |
| `create_channel`, `edit_channel` | C→S | channel mutation |
| `delete_channel` | C→S | `channel_id` |
| `set_channel_admin`, `set_server_role` | C→S | target identity and role state |
| `admin_mute`, `move_user`, `kick_channel`, `kick_server` | C→S | target `client_id`, exact `session_id`, and operation data; a legacy request without `session_id` is accepted only when the identity has exactly one online session |
| `ban`, `unban` | C→S | target `client_id`; ban also carries reason and optional expiry |
| `list_bans` | C→S | paginated `offset` and `limit` (maximum 50) |
| `ban_list` | S→C | authorized page of active bans plus `offset` and `total` |
| `capabilities` | S→C | current server role, authorized action names, and optional `network_policy_channel_ids`; room-policy authorization uses the exact wire name `network.policy.set`, while the channel list lets the UI enforce CHANNEL_ADMIN scope against the target room |
| `request_media_rekey`, `media_rekey_prepare`, `media_rekey_ack`, `media_rekey_commit` | both | coordinated media key epoch transition |

Channels include `id`, optional `parent_id` (zero is root), `name`, `sort_order`, and `network_policy` (`auto`, `force_sfu`, or `force_p2p`). The server persists this policy as channel administration state; an empty room or server restart does not reset it. The server alone decides a session's channel. A deafened state implies muted; clearing deafen does not implicitly unmute. `muted` in snapshots and state-change events is the effective state (`self mute || administrative mute`), while `admin_muted` lets compliant clients keep capture and direct-peer playback closed even if the user requests local unmute.

## UDP packet

All integers use network byte order (big endian). No language struct memory layout is sent.

| Offset | Size | Field |
|---:|---:|---|
| 0 | 2 | Magic `0x4544` |
| 2 | 1 | Version `2` |
| 3 | 1 | Packet type: audio `1`, register `2`, probe `3`, probe ack `4` |
| 4 | 2 | Flags |
| 6 | 8 | Session ID |
| 14 | 4 | Sequence |
| 18 | 4 | Timestamp |
| 22 | 2 | Key epoch |
| 24 | 2 | Payload length |
| 26 | N | ChaCha20-Poly1305 ciphertext and authentication tag |

The maximum datagram is 1500 bytes. A register packet has no payload. Probe payloads contain an 8-byte nonce/timestamp echoed by ProbeAck. Audio sequence increments by one per logical stream packet with unsigned wrap; timestamp increments by 960 for every 20 ms 48 kHz frame and remains continuous across route migrations.

## UDP registration and validation

After TCP welcome, a client sends a register packet from its voice socket. The server records the observed source IP and port; it never trusts a client-reported public address. Audio is accepted only when both session ID and source endpoint match the registration. Routing uses the session's channel from server state, never a channel supplied in the voice packet.

Unknown versions/types, inconsistent lengths, oversized datagrams, unknown sessions, mismatched endpoints, and unregistered sources are dropped. The server may log them at WARN with rate limiting but never reflects data or crashes.

## Versioning

The v0.5 binary voice version is 2 and its fixed header is 26 bytes. Bytes 22-23 contain `key_epoch`; bytes 24-25 contain payload length. The header is authenticated as AEAD AAD and the payload on secure sessions includes a 16-byte ChaCha20-Poly1305 tag. Incompatible versions, invalid epochs, failed authentication, replay, and stale packets are dropped before decode.
