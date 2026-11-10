# React desktop migration

The new desktop host lives in `desktop/`. The original `client/` Slint host remains
runnable until feature parity and real-device acceptance are complete. Both hosts use
the same `edenvoice-client::application::Application`; there is no duplicated network
or audio core.

## Implemented

- Tauri 2 native window, React 19, TypeScript and Vite production build.
- Bounded command queue and immutable `core-state` snapshot event.
- Connection, first-use server fingerprint trust, disconnect and Master Token input.
- Hierarchical channel tree with double-click join and visibly distinct nesting.
- Top-level/sub-channel creation, edit, delete confirmation and scoped network policy.
- Channel chat, member presence, speaking/mute state and avatar rendering.
- Custom avatar selection, drag/zoom crop, 128×128 JPEG and 12 KiB limit.
- Local user volume/mute plus capability-scoped moderation and role actions.
- Audio device, gain, volume, noise suppression, AEC and AGC persistence.
- Saved server profile editing and persistent local identity management.
- Ban-list pagination/unban and a move-member channel picker.
- Output test, device refresh, input-mode switching and microphone original/processed preview.
- Dark, light and system-following appearance persistence.
- Empty-by-default shortcut capture with a 500 ms modifier chord window.
- Browser mock preview for visual work without a server or audio device.

## Remaining parity work

- Complete global-hotkey behavior. PTT remains the only
  globally registered shortcut in Rust; other React shortcuts currently require the
  window to be focused.
- English localization and compact-density persistence.
- Real-client, real-server and physical audio-device acceptance. Browser mock traffic
  is never evidence of audible quality or network correctness.

## Verification

```bash
cd desktop
npm run build
CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```
