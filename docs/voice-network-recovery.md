# VPN switching and room notification regression checks

## Recovery behavior

The desktop application uses `AdaptiveTransport` for remote audio, including
SFU rooms. A successful UDP `send_to` does not prove the server can still send
back to the client. VPN/NAT changes may change the external endpoint without
changing the local interface list. The server deliberately rejects ordinary
audio/probes from an unregistered source.

The receiver now renews the existing authenticated registration every 250 ms
(formerly 12 seconds). A route-local-IP check at the same cadence requests a
fresh socket when the selected local address changes. Rebinding discards old
direct-path qualifications and requests SFU fallback while candidates qualify
again. A temporarily failed socket rebuild retries with a 500 ms delay instead
of terminating the receiver. Existing server protocol and authentication are
unchanged; no server upgrade is required for this fix.

These are receiver-thread operations, not audio callback work. Stable operation
adds at most four registration packets and four local route lookups per second
per connected client. Unchanged registrations do not rebroadcast candidates or
advance candidate generations. This is small additional work, not zero cost.
The 100 ms receive timeout and OS scheduling also affect the actual cadence.

## Automated evidence

- `cd desktop && npm run test:notifications` (Node 24): other-member arrival,
  departure, bulk changes, other-room isolation, reconnect baseline, self-room
  changes, and administrative mute.
- `cd desktop && npm run build`: TypeScript and production UI build.
- `cd client && CMAKE_POLICY_VERSION_MINIMUM=3.5 mise exec -- cargo test --lib`:
  real UDP proxy changes its external endpoint twice while the client only
  receives. Covers plaintext and encrypted media, loss of the first migration
  registration, and socket replacement. The test requires recovery within
  950 ms; this is a local test bound, not an Internet latency guarantee.
- `cd server && mise exec -- go test -race ./internal/voice ./internal/serverstate`:
  secure registration migration, unchanged heartbeat behavior, and rejection of
  forged/replayed registrations, alongside relay/state regression tests.

## Required real-device acceptance

The proxy test cannot reproduce a specific VPN driver's route teardown, audio
device changes, TCP reconnection, or a period where the OS has no working route.

1. Start two clients, join the same room, and confirm the affected client shows
   SFU in diagnostics. Keep the second client speaking continuously.
2. On the affected client, mute the microphone and switch the actual VPN or
   accelerator on and off at least three times. Record the audible gap and
   whether control disconnected, the transport changed, or the audio device
   changed. Repeat once with the microphone active.
3. Repeat with the VPN's alternate routing mode if the original problem used
   one. An underlying network blackout cannot be removed by faster UDP recovery.
4. Have the second client enter/leave the first client's room: one corresponding
   cue should play. Moving within unrelated rooms should not play a cue.
5. Check that self join/leave and administrator mute cues still work, and that
   reconnecting does not play an arrival cue for every existing member.

Do not interpret the local packet recovery measurements as a completed test of
the user's VPN software or as a guarantee of uninterrupted speech during an
OS-level network outage.
