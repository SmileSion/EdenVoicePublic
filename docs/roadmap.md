# Roadmap

## V0.1 — Modern TeamSpeak Core

Server, channels, native GUI, central Relay voice, Opus, audio devices, jitter buffer, mixer, network statistics, and basic reconnection.

## V0.2 — Daily Driver Foundation (in progress)

Formal UI/application/core separation, Apple-inspired design system, System/Light/Dark
themes, professional replaceable audio processing (AEC, AGC, Standard, RNNoise, and
DeepFilterNet), Voice Activity and global Push To Talk input modes, gain and local
mute controls, runtime device recovery, diagnostics, reconnect UX, and documented
manual cross-platform validation. See `docs/v0.2-audit.md` and `docs/audio-processing.md`.
Measured DSP baselines are recorded in `docs/performance.md`.

Deferred from the V0.2 release gate: system tray integration, advanced Bluetooth and
multi-route handling, polished hot-plug UX beyond default fallback, a local benchmark
button, and deeper VAD sensitivity controls. Do Not Disturb is reserved as a distinct
future presence/notification concept; it is not conflated with mute or deafen.

## V0.3 — Adaptive Networking (completed)

SFU-first admission, candidate discovery, qualified P2P optimization, transitional hybrid migration, route qualification, NAT rebinding, and Relay fallback.

## V0.4 — Weak Network and Network Resilience (completed)

Network Quality Engine, lightweight Audio QoE feedback, adaptive and fixed Opus quality, FEC/loss expectation, adaptive jitter buffering, route recovery, room network policy, weak-network UX and deterministic simulation. See `docs/v0.4-network-resilience.md`.

## V0.5 — Security and Administration

Implemented in v0.5: TLS 1.3 control, persistent client/server identities, challenge authentication, encrypted/replay-protected SFU and pairwise P2P media, persistent channels and roles, centralized authorization, mute, move, kick, and ban.

## V0.6 — P2P

UDP hole punching, `P2PTransport`, Relay fallback, and automatic transport selection.

## V0.7 — Advanced Audio Processing

Post-V0.2 adaptive quality work, additional DSP engines where justified by measured
quality, and advanced device-specific tuning.

## V0.8 — Social

Saved servers, favorites, friend identity, and private voice.

## V0.9 — Cross Platform

First-class Windows and Linux packaging and validation.

## V1.0

A stable modern self-hosted voice client.
