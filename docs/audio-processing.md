# Audio Processing

## Current implementation status

V0.2 now has a composable capture-processing boundary in `client/src/dsp.rs`.
The live pipeline is:

```text
CPAL capture callback
  -> device-rate mono PCM
  -> streaming resampler to 48 kHz
  -> bounded raw 48 kHz mono frame queue
  -> manual input gain
  -> EchoCanceller interface
  -> NoiseSuppressor interface
  -> AutomaticGainController interface
  -> RMS/peak analysis
  -> VoiceActivityDetector
  -> Voice Activity or Push To Talk gate
  -> Opus encoder and VoiceTransport
```

The mixer also feeds each rendered 20 ms frame to the echo-canceller render-reference
entry point. Synchronous DSP work runs in the voice worker rather than a CPAL
callback; DeepFilterNet inference runs in its own bounded worker.

`Standard` uses the complete WebRTC AudioProcessing module, including its transient
suppression stage for keyboard, mouse, bottle-cap, and impact sounds. A 20 ms Eden
Voice frame is processed as two preallocated 480-sample/10 ms chunks. WebRTC APM
also supplies AEC3, traditional noise suppression, and AGC2; AEC render reference is
fed from the post-master-mix signal. `Off` disables noise suppression while retaining
independently enabled AEC/AGC through Sonora. `RNNoise` is implemented with
the native Xiph engine bundled by rnnoise2 0.1.2. `DeepFilterNet · Natural` and `DeepFilterNet · Strong` use the
pinned official DFN3 Tract backend and model described below.

Changing input gain, input mode, AEC, AGC, suppression mode, or strength constructs
the replacement processor on a dedicated loader and swaps it with a 100 ms
frame-boundary crossfade. The prior processor remains active until initialization
succeeds, including when a saved DeepFilterNet choice is restored at connection
startup. This does not rebuild the transport session. Device changes rebuild only
the CPAL streams while retaining the queues, worker, codec state, and transport. The
old streams are resumed if replacement startup fails. A runtime device callback
error attempts recovery on the system default for the failed direction and reports
the actual active devices plus fallback state to diagnostics.

Input and output streams prefer 48 kHz, then negotiate the closest rate exposed by
the selected device. The stream callbacks resample between that device rate and the
fixed 48 kHz processing/network format. Hardware sample rates are intentionally not
user-configurable; diagnostics reports both negotiated rates so device compatibility
remains observable.

Device enumeration and named/default selection are exposed through the
`AudioDeviceBackend` trait together with `open_input` and `open_output`. The current
`CpalAudioBackend` supplies the native Windows/macOS/Linux streams. Its lower
`AudioDeviceCatalog` uses an associated device type, allowing discovery/selection to
be covered with a fake implementation without physical hardware.

## Common format

- Internal processing sample rate: 48 kHz
- Hardware stream sample rate: negotiated automatically, preferring 48 kHz
- Channels: mono capture processing
- Network/Opus frame: 960 samples, 20 ms
- APM/RNNoise adapters may split this into two 480-sample, 10 ms blocks internally.
- The UDP and Opus frame interval will not change merely to accommodate DSP.

## Engine capability model

Noise suppression modes are `Off`, `Standard`, `RnNoise`, `DeepFilterNet · Natural`,
and `DeepFilterNet · Strong` and are strictly mutually exclusive. Strength is `Low`,
`Medium`, or `High`, but UI controls are only exposed when the selected implementation
reports real support. An engine without such a parameter reports Automatic and safely
ignores saved strength. New installations default to `Standard` at `Medium` strength.

## Engine selection and remaining evaluation

### Standard, AEC, and AGC

Standard mode uses `tonarino/webrtc-audio-processing-sys` 2.1.0 and statically bundles
the C++ WebRTC APM extraction. Its configuration explicitly enables Transient
Suppression together with the selected Low/Moderate/High noise-suppression level.
Standard mode also always enables WebRTC AEC and AGC; the separate AEC/AGC controls
remain available only for the other suppression modes.
Bundled builds require a C++ compiler, pkg-config, Meson, Ninja, and Rust llvm-tools;
the release workflow installs these on every desktop platform. Sonora remains the
lightweight frontend for AEC/AGC when another suppressor is selected. The current
stream-delay estimate is 60 ms and must eventually be replaced by measured
device/buffer delay. No custom acoustic echo canceller is implemented.

### RNNoise

Implementation: https://github.com/wiiznokes/rnnoise-rs (`rnnoise2 0.1.2`),
using the Xiph RNNoise C sources and the model pinned below.

- Wrapper license: MIT; Xiph RNNoise code license: BSD-3-Clause.
- Format: 48 kHz mono, 480 samples/10 ms per processing call.
- Integration: one long-lived state, two calls per Eden Voice frame, preallocated
  input/output arrays, and normalized-float to RNNoise `i16`-scale conversion.
- Output is always the native RNNoise result; speech probability is diagnostic only
  and never selects a dry bypass.
- Strength capability: Automatic. The core frame API has no genuine stable strength
  parameter, so saved Standard strength is ignored.
- Model archive: `rnnoise_data-0a8755f8e2d834eff6a54714ecc7d75f9932e845df35f8b59bc52a7cfe6e8b37.tar.gz`;
  its filename digest and verified SHA-256 are both
  `0a8755f8e2d834eff6a54714ecc7d75f9932e845df35f8b59bc52a7cfe6e8b37`.
  This is the same model generation used by `pyrnnoise 0.4.3`, not the much
  smaller legacy model previously supplied by `nnnoiseless`.
- Local release builds can prepare the archive from Xiph, verify the SHA-256
  above, and pass its absolute path through `RNNOISE_MODEL_PATH`. The Linux
  container packaging script forwards this path into the container. There is no
  GitHub Actions job downloading or verifying the model for this repository.

## Input gating

Capture records the PTT and mute permission at the callback boundary. The capture
processor carries the source timestamps and permissions with each 10 ms chunk.
DeepFilterNet advances the metadata through its 30 ms model delay inside the model
worker and returns it with the corresponding asynchronous response; rejected requests
do not advance either timeline. This also accounts for the two-frame/40 ms response
pipeline. PTT transmission masks unauthorized chunks, preserving the delayed tail
captured before key release. A current mute still stops transmission immediately.
Processor crossfades mask each side before mixing. Local microphone preview keeps
the complete processed signal and is not cropped by this transmission mask.

The UDP-to-audio handoff records application ingress time and expires packets after
100 ms, even when the queue is not full. Expiry clears only the affected speaker's
pending and decoded audio. A playout worker pause of at least 100 ms clears old
playout before its first resumed mix, including when newer packets have already
evicted all old UDP handoff entries. This is an application queue freshness limit,
not a claim about end-to-end latency or time spent in OS/network buffers.

Receiver QoE uses probe measurements for the path that delivered the authenticated,
deduplicated audio copy. SFU/Relay measurements cover the receiver-to-server leg;
they are not a measurement of the sender's uplink. The optional control field
`path_metrics_available: false` identifies unavailable or expired measurements;
legacy feedback without the field retains its previous interpretation. Transport
packet formats remain unchanged. `receive_stale_drops` in audio timing diagnostics
counts expired handoff packets.

Voice Activity is the default input mode. The energy VAD uses an attack threshold and
ten-frame release/hangover so silence stops network sends without abruptly cutting
sentence endings. Push To Talk uses the same send gate; mute has unconditional
priority over both modes. The network transport only receives frames after the gate
and does not know which input mode produced the decision.

Global PTT currently registers F8 through `global-hotkey 0.8` behind
`platform::hotkey`. Registration and press/release callbacks only update application
and audio-worker state. Failure remains non-fatal and is exposed to the settings UI.
The backend covers Windows, macOS, and Linux/X11. Linux/Wayland needs a separate
desktop-portal adapter and must report the limitation instead of claiming a working
global shortcut.

## Gain system

- Manual input gain is applied in the voice worker before capture DSP.
- Per-user volume is session-local, supports 0–200%, and is applied after each peer's
  decoder and before mixing.
- Local user mute replaces that peer's decoded frame with silence and never reaches
  the server.
- Master output volume is applied after mixing and before both the AEC render-reference
  input and the playback queue.

After per-user gain and summing, the mixer uses a zero-lookahead soft-knee limiter:
samples at or below 0.85 remain unchanged and excess level approaches full scale
smoothly instead of hard clipping. The limiter adds no buffering or algorithmic
latency, preserves finite output, and makes the 100–200% per-user range safer.

The output test generates a bounded 440 Hz, 500 ms tone inside the existing audio
worker with no per-frame allocation. It respects master output volume, plays even
while remote audio is deafened, and is included in the AEC render reference.

The microphone test captures exactly 200 20 ms frame pairs (four seconds) into
pre-sized local buffers. `Original` is copied immediately after the capture queue;
`Processed` is copied after input gain, AEC, the selected noise suppressor, AGC, and
any active processor transition. This records both sides in one pass with only the
selected realtime processor—it does not run RNNoise and DeepFilterNet concurrently.
Processed playback starts automatically, after which the user can replay Original or
Processed at master output volume. Capture is never sent to the server while recording
or playing; playback temporarily replaces remote mix output, is fed to the AEC render
reference, and can be stopped safely. The ready-to-preview state resumes normal send
gating and retains both buffers until cleared or replaced by a new test.

### DeepFilterNet

Upstream: https://github.com/Rikorose/DeepFilterNet

Eden Voice pins the official `deep_filter` v0.5.6 source at commit
`978576aa8400552a4ce9730838c635aa30db5e61`, enabling only `tract` and
`default-model`. The selected bundled model is `DeepFilterNet3_onnx.tar.gz`:

- repository license: MIT or Apache-2.0; Eden Voice selects MIT and reproduces its
  notice in `THIRD_PARTY_NOTICES.md`;
- model archive: 7,983,136 bytes, SHA-256
  `c94d91f70911001c946e0fabb4aa9adc37045f45a03b56008cb0c8244cb63616`;
- format: 48 kHz mono, 960 FFT, 480 hop, two-hop lookahead;
- algorithmic STFT/lookahead delay: 1,440 samples / 30 ms;
- Apple M4 release measurement: 0.2079 ms average, 0.3903 ms peak over 5,000
  20 ms frames, with zero 20 ms deadline misses;
- current release benchmark executable: 25 MB (not a packaged-client size claim).

The source repository states that the repository work is dual-licensed and does not
declare a separate license for its tracked model archives. This conclusion must be
rechecked during release packaging, along with Windows/Linux builds and resident
memory measurements.

The generic lazy-load path is implemented as a dedicated `dsp-loader` thread. Each
request carries a monotonically changing generation; queued requests are coalesced,
stale completed results are dropped, and only the current generation can be swapped
at a frame boundary. Until ready, the previous processor remains active.
DeepFilterNet initializes its Tract model inside a dedicated inference thread because
the upstream runtime is intentionally not `Send`. The realtime-facing adapter uses
sequence-tagged bounded request/response queues and a fixed two-frame (40 ms)
pipeline. Together with the model's 30 ms STFT/lookahead delay, DeepFilterNet adds
about 70 ms of DSP latency before device/playout buffering. A capture frame can only be replaced by the response carrying its exact
expected sequence. Startup and deadline misses substitute silence; unprocessed
current audio and stale processed frames are never allowed through. Inference
results are read without waiting: a delayed neural worker cannot take another
20 ms playback period from the shared audio thread. Inference durations still feed diagnostics. The Natural profile limits attenuation to 18 dB,
disables the post-filter, and uses local-SNR thresholds `-15/35/25 dB`. The Strong
profile allows full attenuation, enables a `0.02` post-filter, and uses
`-5/40/30 dB`, favoring voice isolation over timbral naturalness. Initialization
failure keeps the voice session and previous processor alive. Switching away drops
both bounded channels and detaches the dedicated inference thread without waiting
on the audio thread. The inference thread releases its model when its current
inference (or next queued frame if idle) encounters the disconnected response
channel. Model memory can temporarily overlap during a switch; no model cache is
retained. Initialization failures still join outside realtime processing.

## Metrics

The DSP boundary records processed frames, total/average processing time, peak time,
and processing calls exceeding the 20 ms network-frame deadline. The settings
diagnostics view shows the actual active backend and these measurements. It does not
invent a CPU percentage.

The same diagnostics view reports connection state, RTT, sent/received/lost and
reordered packets, loss percentage, aggregate per-peer jitter-buffer depth, Opus
bitrate, input RMS/peak, output RMS, active AEC/AGC state, and cumulative capture
queue overruns and playback queue underruns. The realtime callbacks update only
atomics or bounded queues; formatting and presentation remain on the UI thread.

The worker also evaluates one-second metric deltas. Only three consecutive intervals
with at least 10 ms average processing time or three deadline misses produce a
non-fatal warning. A healthy interval clears the streak. The guard never silently
changes the user's selected engine; it recommends a lighter mode while preserving
the voice session and active processor.

## License gate

The pinned Xiph RNNoise model and native source have the explicit conclusion recorded above.
DeepFilterNet's default model is tracked in the upstream repository, whose project
files are offered under MIT or Apache-2.0; Eden Voice selects the MIT option and
records the exact revision, archive hash, and attribution in `THIRD_PARTY_NOTICES.md`.
Release packaging must preserve those notices and recheck that the pinned upstream
revision has not introduced a model-specific exception.
