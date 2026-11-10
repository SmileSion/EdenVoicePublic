# EdenVoice Desktop

This directory is the Tauri 2 + React desktop host for the existing Rust client core.
The current source version is **0.8.5**; published installer versions may differ.

- `src/` contains the React/TypeScript interface and a browser-only mock snapshot.
- `src-tauri/` owns the native window and the bounded command/snapshot bridge.
- `../client/` remains the source of truth for networking, identity, audio, DSP,
  persistence, protocol handling, and application state.

## Development

```bash
npm install
npm run dev
```

The browser preview uses `src/mock.ts`; commands are logged rather than sent to a
server. Use it for fast visual iteration.

```bash
npm run tauri dev
```

The Tauri build runs the real Rust Core and requires CMake, Meson, Ninja and pkg-config.
On macOS, install them with `brew install cmake meson ninja pkg-config`. The launcher
sets the bundled libopus CMake compatibility policy automatically.

## Release packages

Use the repository-level script rather than calling Tauri directly:

```bash
./scripts/package-release.sh --platform macos
./scripts/package-release.sh --platform windows
./scripts/package-release.sh --platform linux
```

On macOS, the Windows command performs a local x64 MSVC cross-build with cargo-xwin
and produces an NSIS installer. See the root README for one-time prerequisites and
the optional offline NSIS helper setting.

## Boundary rules

- JavaScript may send only tagged `CoreCommand` values.
- Rust serializes IDs as strings and publishes immutable `CoreSnapshot` values.
- Audio frames, sockets, protocol envelopes, and cryptographic material stay in Rust.
- Browser mock behavior must never be treated as proof that a real core command works.

## Screen sharing

Sharing is viewed in the room's screen tab, with an optional full-screen view.
Members choose to watch; switching tabs keeps the stream alive, while stopping
watching or leaving the room releases it. Each room has one sharer, and separate
rooms can share concurrently within the server's configured limits.

Server v0.8.5 uses a single configurable UDP port for WebRTC media (`7010` by
default), separate from voice traffic. The client learns this port through SDP;
existing screen-sharing-capable clients need no update for this server change.
See the [configuration and testing guide](../docs/screen-sharing.md) for deployment
and platform capture limitations.
