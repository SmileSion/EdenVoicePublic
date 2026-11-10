# External mpv watch party

Current moderation and UX: [2026-09-07 revision](watch-party-moderation-2026-09-07.md). The dated implementation history below predates host/admin-only control.

EdenVoice owns the synchronization protocol; this is not Syncplay wire compatibility.
The Go server is authoritative. Each voice room has an optional watch session.
Every viewer opens a local copy in an external mpv process launched by EdenVoice.
Only playback metadata crosses the network; never local paths or video bytes.

## Acceptance checklist

- [x] Go room state, revision ordering, finite/bounded position validation and tests.
- [x] Authenticated control protocol, room isolation, leave/move/disconnect cleanup.
- [x] Client protocol and lifecycle; older servers do not enable watch controls.
- [x] Configurable mpv executable, local file selection, owned process cleanup.
- [x] Unix socket and Windows named pipe JSON IPC, bounded reads and timeouts.
- [x] Play/pause/seek synchronization without command echo; drift correction.
- [x] Participant readiness and file mismatch warnings; late join synchronization.
- [x] Room UI, error reporting, independent volume/subtitle/audio-track settings.
- [x] Tests/builds and documented platform-runtime verification limitations.

## Invariants

No changes to audio callbacks, DSP or UDP media loops. No inactive watch polling.
Watch state has its own lock; network sends never hold the media state lock.
Only the mpv child launched for this watch session is controlled or terminated.
Closing mpv leaves watching, not voice. Changing voice rooms leaves watching.
Remote messages map to a fixed playback command allowlist, never arbitrary IPC.
File metadata comparison is a warning, not proof of byte-identical content.

## Server wire contract

Feature: `watch_party_v1`. Client sends `watch_request` with `channel_id`,
`epoch`, `request_id`, `action`, optional `file`/`ready` or `position`.
Actions: join, leave, file, play, pause, seek, sync. Join returns an epoch;
subsequent actions must use it. Server returns `watch_state` with authoritative
position/paused, epoch/revision, participants, actor/action, and request ID.
Errors are watch-local responses, not connection failures. Sync replies only
to the requesting viewer; transitions broadcast only to current viewers.
Ignore stale epochs/revisions; match request IDs together with actor ID.
Explicit leave response is distinct from another viewer's `member_left` event.

At most 64 viewers, file display name at most 128 UTF-8 bytes (client must
truncate safely); local paths never sent. Clients coalesce IPC events and space
requests at least 40 ms apart; normal periodic sync should be far less frequent.
No server watch ticker. Position is extrapolated from monotonic elapsed time.

Verified: `mise exec -- go test -race ./internal/control ./internal/watchparty
./internal/protocol` passed, including non-viewer rejection, move/disconnect
cleanup, stale epoch rejection and worst-case JSON snapshot size.

Desktop verification includes Rust unit tests, a spawned fake mpv speaking the
real newline-framed Unix JSON IPC protocol (including fragmented responses),
the React/native-dispatch browser test, full client regression, frontend build,
and a Windows x64 cross-compile of the named-pipe branch. This machine has no
real mpv installation, so decoding/rendering and Windows named-pipe runtime
remain explicit platform acceptance tests rather than automated evidence.


## v0.6.8 watch-room revision (2026-09-05)

The updated desktop requires `watch_rooms_v1`; the server still advertises
`watch_party_v1` for earlier clients. Open/join a room before selecting media.
A room has a host (creator, then lowest remaining session ID on host departure).
Only the host may close it. `closed` is broadcast to every viewer before control
membership/rate state is removed. Reopening gets a new epoch. `status` is a
read-only query for users in the same voice channel; it does not create a room.
`unready` clears old metadata/readiness while replacing or closing a player.
The panel polls discovery every two seconds only while open; participants sync
at the existing cadence even before loading a file. Closing a player retains
room membership for retry. Voice room changes/disconnect still remove watching.

Player configuration now lives under Settings → Player. `save_mpv_path` waits
for validated config persistence and reports write failures; configuration is
not rewritten by incoming playback events. Local selected filenames remain
visible during startup and after failure. No local media paths are sent over
the wire. Settings save immediately, independently of the general settings
footer. The old `set_mpv_path` command remains compatible.

Windows root cause: std File::read maps ERROR_NO_DATA (232) via BrokenPipe to
Ok(0). A PIPE_NOWAIT read while idle was treated as EOF and dropped the owned
child. The new Windows branch waits for child exit or the bounded IPC timeout.
See Rust 1.95 sources `std/src/sys/io/error/windows.rs` and
`std/src/sys/pal/windows/handle.rs`, and Microsoft's
[pipe wait modes](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-type-read-and-wait-modes).
Windows canonical verbatim paths are normalized for the media command line,
including UNC paths. mpv uses a forced window, paused start, and bounded local
stderr diagnostics; startup, decoder and timeout errors remain visible.

Verification: Go race tests include discovery without membership allocation,
non-host close rejection, close broadcast/cleanup, fresh epochs and host
transfer. React browser tests exercise settings save/remount, selected file,
failed-launch retry, playback, host controls and unsupported-server gating.
Rust tests cover config round-trip, Windows media path conversion, a spawned
fake IPC player with fragmented reads, and early exit reporting before Closed.
Windows named-pipe execution and real decoder/rendering still require Windows
acceptance; the Windows release is cross-compiled on macOS, not CI-built.

## Moderation revision (2026-09-07)

The server additionally advertises `watch_moderation_v1`. Each `watch_state`
contains recipient-specific `permissions` (`role`, `can_control`, `can_close`,
`can_transfer_host`). Host and same-channel authorized administrators can close
or transfer without watch membership; play/pause/seek require ready membership.
`transfer_host` carries `target_session_id`, which must be a current participant.
All management requests use the current epoch. Nonparticipating administrators
receive a direct acknowledgement; broadcasts remain scoped to watch members.
Ordinary viewers retain local file selection and read-only sync. The desktop
stops generating global commands from their mpv playback events.
