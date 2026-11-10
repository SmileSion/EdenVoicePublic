# Performance Baseline

This document separates measurements from release gates. A dash means the scenario
has not been measured; it must not be interpreted as zero.

## V0.5 secure-media microbenchmarks — 2026-08-25

- Platform: Apple M5, macOS arm64
- Go: mise-managed Go 1.23
- Command: `cd server && mise exec -- go test ./internal/security ./internal/voice -run '^$' -bench 'Benchmark(MediaSealOpen|SecureSFUDecryptAndFiveWayFanout)$' -benchmem -count=3`

| Workload | Time/op (3 runs) | Throughput | Memory/op | Allocations/op |
|---|---:|---:|---:|---:|
| Media seal + open | 536–567 ns | — | 304 B | 4 |
| Secure SFU decrypt + five-recipient fanout | 1.914–1.916 µs | 62.62–62.71 MB/s | 1,104 B | 14 |

These microbenchmarks cover the symmetric secure-media hot path. They do not replace
real-device end-to-end latency, long-session memory, weak-network recovery, or audible
SFU/P2P migration checks.

### Post key-lifecycle regression check — 2026-08-25

After adding explicit disconnect/rekey key destruction, the same Apple M5 ran a longer
three-second-per-sample check:

`cd server && mise exec -- go test ./internal/security ./internal/voice -run '^$' -bench 'Benchmark(MediaSealOpen|SecureSFUDecryptAndFiveWayFanout)$' -benchmem -benchtime=3s -count=3`

| Workload | Time/op (3 runs) | Throughput | Memory/op | Allocations/op |
|---|---:|---:|---:|---:|
| Media seal + open | 575.2–608.1 ns | — | 304 B | 4 |
| Secure SFU decrypt + five-recipient fanout | 1.906–1.952 µs | 61.47–62.96 MB/s | 1,104 B | 14 |

The SFU fanout range overlaps the original result and both workloads retain exactly the
same allocation counts. The isolated seal/open measurement is roughly 7% slower than the
earlier short sample (about 40 ns absolute), while remaining below one microsecond per
packet pair; no corresponding five-recipient hot-path regression was reproduced.

## DSP microbenchmark — 2026-09-02

- Platform: MacBook Air (Mac16,12)
- CPU: Apple M4, 10 cores (4 performance, 6 efficiency)
- RAM: 24 GB
- OS: macOS 26.4.1 (25E253), arm64
- Build: Rust release profile
- Workload: 5,000 deterministic synthetic 48 kHz mono frames after 100 warm-up
  frames; 960 samples/20 ms per frame
- Command: `cd client && cargo run --release --example dsp_benchmark`

| Engine | Frames | Average DSP ms | Peak DSP ms | Real-time factor | >20 ms |
|---|---:|---:|---:|---:|---:|
| Off | 5,000 | 0.0039 | 0.0109 | 0.0002 | 0 |
| Standard + AEC + AGC | 5,000 | 0.2378 | 0.2873 | 0.0119 | 0 |
| RNNoise + AEC + AGC | 5,000 | 1.0264 | 5.0094 | 0.0513 | 0 |
| DeepFilterNet | 5,000 | 0.2079 | 0.3903 | 0.0104 | 0 |

The real-time factor is elapsed processing time divided by represented audio time;
lower is better. This synthetic benchmark proves timing only on the machine above.
It does not prove denoising quality, echo cancellation quality, end-to-end latency,
or results on Windows/Linux/other CPUs.

## AI denoise matrix

| Property | RNNoise | DeepFilterNet target |
|---|---|---|
| Quality | Good target; real-noise acceptance pending | Higher target; real-noise acceptance pending |
| CPU | 1.0264 ms/frame for the newer pinned Xiph model | 0.2079 ms/frame for direct model inference in this synthetic probe |
| Peak probe RSS | 22.95 MiB; +13.52 MiB versus the same-build Off probe | 67.38 MiB; +63.81 MiB versus Off in the prior probe |
| Algorithmic frame | 480 samples / 10 ms | 480 samples / 10 ms; 30 ms model/STFT delay |
| 48 kHz | Yes | Yes |
| Long gaming session | Preferred lightweight mode | Device-dependent |

## DSP memory probe — 2026-08-20

The newer pinned Xiph RNNoise model was remeasured on 2026-09-02 using the same
Apple M4 and release command described above. The same-build Off probe reached
9.44 MiB maximum RSS and RNNoise + AEC + AGC reached 22.95 MiB, an incremental
13.52 MiB. Absolute RSS should not be compared directly with the older build below;
the incremental result isolates the replacement in the current executable.

The release `dsp_memory_probe` executable loads exactly one processing mode, processes
200 synthetic frames with a 1 ms scheduling gap, drops the processor, and exits.
`/usr/bin/time -l` measures maximum resident set size for a separate process per mode:

| Mode | Maximum RSS | Delta versus Off |
|---|---:|---:|
| Off | 3.56 MiB | — |
| Standard + AEC + AGC | 5.03 MiB | +1.47 MiB |
| RNNoise + AEC + AGC | superseded | remeasurement required after the newer Xiph model replacement |
| DeepFilterNet + AEC + AGC | 67.38 MiB | +63.81 MiB |

Commands:

```bash
cd client
cargo build --release --example dsp_memory_probe
/usr/bin/time -l target/release/examples/dsp_memory_probe off
/usr/bin/time -l target/release/examples/dsp_memory_probe standard
/usr/bin/time -l target/release/examples/dsp_memory_probe rnnoise
/usr/bin/time -l target/release/examples/dsp_memory_probe deep-filter-net
```

This isolates the incremental model/runtime footprint within one linked executable.
It is not the GUI client's idle/voice RSS, does not prove that the system allocator
immediately returns pages after unload, and does not replace the long-session memory
gate below. The DeepFilterNet adapter nevertheless closes both channels and joins its
model-owning worker during drop, so no model thread or intentional cache survives a
completed processor switch.

## End-to-end scenario matrix

These scenarios require a signed/packaged client and real audio hardware. They remain
release gates rather than guessed values.

| Scenario | CPU | RAM | RTT | Jitter | Loss | Underrun/overrun |
|---|---:|---:|---:|---:|---:|---:|
| Idle client | — | — | N/A | N/A | N/A | — |
| Connected, no voice | — | — | — | — | — | — |
| 1 remote speaker | — | — | — | — | — | — |
| 3 remote speakers | — | — | — | — | — | — |
| Standard + AEC + AGC | — | — | — | — | — | — |
| RNNoise + AEC + AGC | — | — | — | — | — | — |
| DeepFilterNet | — | — | — | — | — | — |

## Five-participant pipeline stress — 2026-08-25

- Platform: Apple M5, macOS arm64, Rust debug test profile
- Workload: five independent real Opus encoders and five `RemotePeer` decoder/jitter
  states feeding the production mixer for 15,000 logical 20 ms frames (five simulated
  minutes, 75,000 encoded/decoded participant frames)
- Command: `cd client && cargo test five_peer_opus_pipeline_stays_bounded_for_five_simulated_minutes -- --ignored --nocapture`

| Decode errors | Maximum jitter depth | 20 ms deadline misses | Peak five-peer frame time |
|---:|---:|---:|---:|
| 0 | 3 frames | 0 | 2.462041 ms |

The stress runner asserts that every expected mixed frame is produced, every sample stays
finite and bounded, jitter state does not grow beyond the configured prefill, and fewer
than three deadline misses can occur consecutively. This closes a deterministic queue
growth and sustained-processing-pressure gap, but it does not exercise CoreAudio callbacks,
real devices, GUI idle CPU, allocator RSS trends, or audible quality; the real-session gate
below therefore remains mandatory.

## Release gates

- No sustained DSP processing over the 20 ms frame deadline.
- No continuous audio glitches or callback queue starvation.
- No increasing memory trend in a long gaming session.
- No abnormal idle CPU activity.
- Repeat the DSP benchmark manually on the release architectures and record results rather
  than extrapolating this Apple M4 measurement.
