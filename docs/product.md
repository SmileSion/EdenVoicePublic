# EdenVoice product

EdenVoice is a modern, low-distraction voice client for friends, small teams, LANs, and self-hosted servers. Its core journey is deliberately short:

```text
Connect → inspect channels → join a channel → talk
```

## V0.1 — Modern TeamSpeak Core

V0.1 provides server connections, nicknames, channel and user presence, join/leave, mute/deafen, device selection, Opus voice over UDP Relay, jitter handling, mixing, basic network statistics, reconnect behavior, and a native desktop GUI. The Go server needs no account service or database and is distributed as one binary and a Docker image.

Voice quality, reliability, and channel clarity take priority over social features. V0.1 specifically excludes accounts, friends, text chat, file sharing, screen sharing, video, bots, P2P, NAT traversal, a permission suite, and end-to-end encryption.

## V0.2 — Daily Driver Foundation

V0.2 preserves that voice core while adding a formal UI/application/core boundary,
a restrained cross-platform design system, replaceable realtime capture processing,
four selectable noise-suppression modes, VAD/global PTT, local gain controls, runtime
device recovery, mic/output tests, and actionable diagnostics. Learned processing is
lazy-loaded off the realtime path and never silently replaces the user's selected
mode after a performance warning.

## Product principles

- Voice first: PCM is encoded with Opus and carried over UDP; control data never shares the voice stream.
- Server centric: the server is authoritative for sessions, channels, and presence.
- Self hosted: simple configuration, no database, and straightforward Linux/Docker deployment.
- Low latency: 48 kHz mono audio in 20 ms frames, short buffering, and no blocking work in audio callbacks.
- Modern UX: native, dark-first, state-driven UI without legacy tree widgets or feature clutter.
