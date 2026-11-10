# Architecture

## Server architecture

The Go server has two independent listeners sharing a thread-safe `ServerState`:

```text
TCP control → control dispatcher → authoritative ServerState
                                      ↑          ↓
UDP voice  → packet validation → session/channel lookup → UDP Relay
```

The control server owns session lifecycle and broadcasts state events. `ServerState` synchronizes channels, sessions, heartbeat timestamps, mute/deafen presence, and observed UDP endpoints. The Relay validates the packet header, session, and source endpoint, derives the channel from server state, and forwards the original datagram only to non-deafened peers in that channel. It never decodes or mixes Opus and never creates a goroutine per packet.

## Client architecture

```text
React UI → Tauri command queue → Application → Dispatcher → control/audio adapters
    ↑                              ↓                         ↓
core-state event ← serializable snapshot ← AppState ← control/audio worker events

audio callback ↔ bounded/ring buffers ↔ codec/peer pipeline ↔ UDP transport
```

The Tauri host owns `Application` on a dedicated Rust worker thread. React receives a
camel-case, immutable snapshot through the `core-state` event and emits tagged,
high-level commands through a bounded queue. IDs cross the WebView boundary as strings
to preserve full `u64` precision. Raw PCM/Opus frames, sockets, control envelopes,
identity keys, media keys, CPAL handles, and transport objects never enter JavaScript.

The GUI does not assume a server action succeeded: channel moves, moderation, chat,
network policy, and connection state render from the next authoritative `AppState`
snapshot. `Application` owns persistence and state reduction; `Dispatcher` adapts
application commands/events to the control and audio workers. A dedicated control
thread performs TCP/NDJSON I/O, heartbeat, and reconnect. The original Slint host
remains runnable during migration as a parity reference, but is not a second core.

## Control flow

The client opens TLS 1.3 over TCP, performs the persistent identity challenge and ephemeral session-key handshake, then waits for `WELCOME` plus `SERVER_STATE`. Joining a channel emits `JOIN_CHANNEL`; the UI changes only after `USER_MOVED`. Presence changes are broadcast to every session. PING/PONG measures RTT and refreshes the server heartbeat deadline.

## UDP voice flow

After `WELCOME`, the client binds UDP and sends `UDP_REGISTER` from the socket it will use for voice. Capture produces 960-sample mono frames at 48 kHz, Opus encodes each frame, and `RelayTransport` attaches sequence and RTP-style timestamp values. The server forwards the unchanged packet to members of the sender's authoritative channel.

## Audio pipeline

```text
input device → capture callback → ring buffer → 20 ms framing → Opus encoder → RelayTransport
RelayTransport → packet dispatcher → per-peer jitter buffer → per-peer Opus decoder/PLC → mixer → output ring buffer → output device
```

Each remote peer owns one decoder, jitter buffer, talking deadline, and packet
statistics. CPAL callbacks only convert/downmix or duplicate samples and exchange
fixed 960-sample arrays through bounded non-blocking queues. The worker thread
performs capture DSP, VAD/PTT send gating, Opus, UDP, peer management, PLC, per-user
gain/local mute, statistics, mixing, and master output gain. The playback mixer sums
in floating point and applies a lightweight soft limiter after volume; its final
frame is also the AEC render reference. Device control, network, JSON, logging,
allocation-heavy work, and blocking mutexes stay outside real-time callbacks.

Device discovery and selection enter through the `AudioDeviceBackend` boundary.
`CpalAudioBackend` is the Windows/macOS/Linux implementation and owns list/open input
and output operations. Application and UI code consume stable device names and
`DeviceList`, while CPAL handles remain inside the audio/core side. The lower
`AudioDeviceCatalog` associated-type boundary makes discovery testable without
physical CPAL devices. This preserves the working streams without spreading host API
assumptions into settings or application state.

## State flow

`ServerState` is the network truth. Client `AppState` is a projection containing connection state, server metadata, ordered channels, users, the local session, audio state, talking deadlines, and network counters. Reconnection repeats HELLO, UDP registration, and the user's most recently requested channel join; the UI still waits for the new server session's `USER_MOVED` event before displaying that move.

## V0.3 adaptive media routing

```text
encoded Opus frame → AdaptiveTransport → Route Manager → P2P / SFU / Relay
                                             ↑
TCP control ← candidate exchange + quality report + room topology evaluator
```

Every join starts on the packet SFU. Candidate checks run concurrently and never gate Audio Ready. A direct path must pass the continuous qualification window, and the server requires a complete room connectivity matrix before switching to P2P. During a P2P-room membership change, `TRANSITIONAL_HYBRID` sends on direct routes and the still-warm SFU path; stream ID/sequence duplicate suppression makes this make-before-break overlap safe. Explicit degradation restores SFU quickly and starts a cooldown before another upgrade.

The media plane treats every session ID as an independent publisher stream and supports explicit subscriber sets. Room membership and UDP endpoint binding are authoritative on the server, preventing cross-room subscriptions and media injection. Atomic packet/byte counters and room/session counts are logged periodically without decoding Opus or taking a global media lock per recipient.
