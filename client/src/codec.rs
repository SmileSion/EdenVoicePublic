use audiopus::{
    Application, Bandwidth, Bitrate, Channels, MutSignals, SampleRate, Signal,
    coder::{Decoder, Encoder},
    packet::Packet,
};

use crate::network::AudioAdaptationDecision;

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: u16 = 1;
pub const FRAME_SAMPLES: usize = 960;
pub const DEFAULT_BITRATE: i32 = 72_000;
const MAX_OPUS_PAYLOAD: usize = 1_276;

/// Inspect the LBRR flag for our single-frame, mono, 20 ms wire format.
/// RFC 6716 sections 3.1 and 4.2.3: one VAD bit precedes the LBRR bit.
/// Other packet layouts are conservatively treated as unconfirmed recovery.
pub(crate) fn packet_has_voice_fec(payload: &[u8]) -> bool {
    if payload.len() < 3 || payload[0] & 0x87 != 0 {
        return false;
    }
    let config = payload[0] >> 3;
    let twenty_ms = matches!(config, 1 | 5 | 9 | 13 | 15);
    twenty_ms && payload[1] & 0x40 != 0
}

pub struct OpusEncoder {
    inner: Encoder,
    output: Vec<u8>,
}

impl OpusEncoder {
    pub fn new(bitrate: i32) -> audiopus::Result<Self> {
        let mut inner = Encoder::new(SampleRate::Hz48000, Channels::Mono, Application::Voip)?;
        inner.set_bitrate(Bitrate::BitsPerSecond(bitrate))?;
        inner.set_complexity(10)?;
        inner.set_vbr(true)?;
        inner.set_vbr_constraint(false)?;
        inner.set_signal(Signal::Voice)?;
        inner.set_max_bandwidth(Bandwidth::Fullband)?;
        inner.set_lsb_depth(24)?;
        Ok(Self {
            inner,
            output: vec![0; MAX_OPUS_PAYLOAD],
        })
    }

    pub fn encode(&mut self, frame: &[f32]) -> audiopus::Result<&[u8]> {
        if frame.len() != FRAME_SAMPLES {
            return Err(audiopus::Error::Opus(audiopus::ErrorCode::BadArgument));
        }
        let length = self.inner.encode_float(frame, &mut self.output)?;
        Ok(&self.output[..length])
    }

    pub fn apply_adaptation(&mut self, decision: AudioAdaptationDecision) -> audiopus::Result<()> {
        self.inner.set_bitrate(Bitrate::BitsPerSecond(
            decision.target_bitrate_bps.clamp(6_000, 510_000),
        ))?;
        self.inner.set_inband_fec(decision.fec_enabled)?;
        self.inner
            .set_packet_loss_perc(decision.expected_packet_loss_percent.min(100))?;
        self.inner.set_dtx(decision.dtx_enabled)
    }
}

pub struct OpusDecoder {
    inner: Decoder,
    silk_state: bool,
}

impl OpusDecoder {
    pub fn new() -> audiopus::Result<Self> {
        Ok(Self {
            inner: Decoder::new(SampleRate::Hz48000, Channels::Mono)?,
            silk_state: false,
        })
    }

    pub fn decode(
        &mut self,
        payload: Option<&[u8]>,
        output: &mut [f32; FRAME_SAMPLES],
    ) -> audiopus::Result<usize> {
        let packet = payload.map(Packet::try_from).transpose()?;
        let decoded =
            self.inner
                .decode_float(packet, MutSignals::try_from(output.as_mut_slice())?, false)?;
        if let Some(payload) = payload {
            self.silk_state = payload[0] & 0x80 == 0;
        }
        Ok(decoded)
    }

    pub(crate) fn can_recover_voice_fec(&self, payload: &[u8]) -> bool {
        // libopus falls back to PLC when transitioning from a CELT decoder,
        // even if the next SILK/hybrid packet contains an LBRR flag.
        self.silk_state && packet_has_voice_fec(payload)
    }

    pub fn decode_fec(
        &mut self,
        next_payload: &[u8],
        output: &mut [f32; FRAME_SAMPLES],
    ) -> audiopus::Result<usize> {
        self.inner.decode_float(
            Some(Packet::try_from(next_payload)?),
            MutSignals::try_from(output.as_mut_slice())?,
            true,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    /// Run with EDENVOICE_SPEECH_PCM pointing to mono 48 kHz signed 16-bit LE PCM.
    /// Keeps third-party speech out of the repository and uses repeatable loss patterns.
    #[test]
    #[ignore = "requires an external real-speech PCM fixture"]
    fn real_speech_fec_matrix() {
        let path = std::env::var("EDENVOICE_SPEECH_PCM").expect("set EDENVOICE_SPEECH_PCM");
        let bytes = std::fs::read(path).unwrap();
        let samples: Vec<f32> = bytes
            .chunks_exact(2)
            .map(|s| f32::from(i16::from_le_bytes([s[0], s[1]])) / 32768.0)
            .collect();
        assert!(samples.len() >= SAMPLE_RATE as usize * 5);
        println!(
            "kbps,bandwidth,expected_loss,fec,loss_pattern,packets,lost,recovered,plc,wire_kbps"
        );
        for bitrate in [16_000, 24_000, 32_000, 48_000, 72_000, 96_000] {
            for wideband in [false, true] {
                for expected_loss in [5, 10, 20] {
                    for fec in [false, true] {
                        let mut encoder = OpusEncoder::new(bitrate).unwrap();
                        encoder
                            .apply_adaptation(AudioAdaptationDecision {
                                target_bitrate_bps: bitrate,
                                fec_enabled: fec,
                                expected_packet_loss_percent: expected_loss,
                                dtx_enabled: false,
                            })
                            .unwrap();
                        encoder
                            .inner
                            .set_bandwidth(if wideband {
                                Bandwidth::Wideband
                            } else {
                                Bandwidth::Auto
                            })
                            .unwrap();
                        let packets: Vec<Vec<u8>> = samples
                            .chunks_exact(FRAME_SAMPLES)
                            .map(|frame| encoder.encode(frame).unwrap().to_vec())
                            .collect();
                        for pattern in ["random5", "random10", "random20", "burst3"] {
                            let mut seed = 7_u32;
                            let loss: Vec<bool> = (0..packets.len())
                                .map(|index| {
                                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                                    index > 5
                                        && match pattern {
                                            "random5" => seed % 100 < 5,
                                            "random10" => seed % 100 < 10,
                                            "random20" => seed % 100 < 20,
                                            _ => index % 30 < 3,
                                        }
                                })
                                .collect();
                            let mut decoder = OpusDecoder::new().unwrap();
                            let mut frame = [0.0; FRAME_SAMPLES];
                            let mut recovered = 0;
                            let mut lost = 0;
                            for (index, packet) in packets.iter().enumerate() {
                                if loss[index] {
                                    lost += 1;
                                    if index + 1 < packets.len()
                                        && !loss[index + 1]
                                        && decoder.can_recover_voice_fec(&packets[index + 1])
                                    {
                                        decoder
                                            .decode_fec(&packets[index + 1], &mut frame)
                                            .unwrap();
                                        recovered += 1;
                                    } else {
                                        decoder.decode(None, &mut frame).unwrap();
                                    }
                                } else {
                                    decoder.decode(Some(packet), &mut frame).unwrap();
                                }
                                assert!(frame.iter().all(|sample| sample.is_finite()));
                            }
                            let wire = packets.iter().map(Vec::len).sum::<usize>() as f64 * 8.0
                                / (packets.len() as f64 * 20.0);
                            println!(
                                "{},{},{expected_loss},{fec},{pattern},{},{lost},{recovered},{},{wire:.1}",
                                bitrate / 1000,
                                if wideband { "wide" } else { "auto" },
                                packets.len(),
                                lost - recovered
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn fec_detection_rejects_celt_dtx_and_other_packet_layouts() {
        assert!(!packet_has_voice_fec(&[]));
        assert!(!packet_has_voice_fec(&[0x08]));
        assert!(!packet_has_voice_fec(&[0x08, 0x40]));
        assert!(!packet_has_voice_fec(&[0xf8, 0xff, 0xfe]));
        assert!(!packet_has_voice_fec(&[0x09, 0xff, 0xff]));
        assert!(!packet_has_voice_fec(&[0x0c, 0xff, 0xff]));
        assert!(!packet_has_voice_fec(&[0x00, 0xff, 0xff]));
        assert!(!packet_has_voice_fec(&[0x08, 0x80, 0]));
        assert!(packet_has_voice_fec(&[0x08, 0x40, 0]));
        assert!(packet_has_voice_fec(&[0x68, 0xc0, 0]));
    }

    #[test]
    fn real_silk_redundancy_recovers_audio_and_celt_transition_does_not_claim_fec() {
        let mut encoder = OpusEncoder::new(24_000).unwrap();
        encoder.inner.set_bandwidth(Bandwidth::Wideband).unwrap();
        encoder
            .apply_adaptation(AudioAdaptationDecision {
                target_bitrate_bps: 24_000,
                fec_enabled: true,
                expected_packet_loss_percent: 20,
                dtx_enabled: false,
            })
            .unwrap();
        let mut decoder = OpusDecoder::new().unwrap();
        let mut output = [0.0; FRAME_SAMPLES];
        let mut recovered = 0;
        for index in 0..100 {
            let pcm: [f32; FRAME_SAMPLES] = std::array::from_fn(|sample| {
                let t = (index * FRAME_SAMPLES + sample) as f32 / SAMPLE_RATE as f32;
                (TAU * 180.0 * t).sin() * 0.15 + (TAU * 540.0 * t).sin() * 0.05
            });
            let packet = encoder.encode(&pcm).unwrap().to_vec();
            if index % 10 == 8 {
                continue;
            }
            if index % 10 == 9 && decoder.can_recover_voice_fec(&packet) {
                assert_eq!(
                    decoder.decode_fec(&packet, &mut output).unwrap(),
                    FRAME_SAMPLES
                );
                assert!(output.iter().all(|x| x.is_finite()));
                recovered += 1;
            }
            decoder.decode(Some(&packet), &mut output).unwrap();
        }
        assert!(
            recovered > 0,
            "test never generated genuine LBRR redundancy"
        );
        let mut celt_encoder = OpusEncoder::new(DEFAULT_BITRATE).unwrap();
        let celt = celt_encoder.encode(&[0.0; FRAME_SAMPLES]).unwrap();
        assert_ne!(celt[0] & 0x80, 0);
        decoder.decode(Some(celt), &mut output).unwrap();
        assert!(!decoder.can_recover_voice_fec(&[0x08, 0x40, 0]));
    }

    #[test]
    fn opus_round_trip_and_plc() {
        let input: Vec<f32> = (0..FRAME_SAMPLES)
            .map(|sample| (sample as f32 * 440.0 * TAU / SAMPLE_RATE as f32).sin() * 0.25)
            .collect();
        let mut encoder = OpusEncoder::new(DEFAULT_BITRATE).unwrap();
        let payload = encoder.encode(&input).unwrap().to_vec();
        assert!(!payload.is_empty());
        assert!(payload.len() < MAX_OPUS_PAYLOAD);
        let mut decoder = OpusDecoder::new().unwrap();
        let mut output = [0.0; FRAME_SAMPLES];
        assert_eq!(
            decoder.decode(Some(&payload), &mut output).unwrap(),
            FRAME_SAMPLES
        );
        assert!(output.iter().all(|sample| sample.is_finite()));
        let mut concealed = [0.0; FRAME_SAMPLES];
        assert_eq!(decoder.decode(None, &mut concealed).unwrap(), FRAME_SAMPLES);
        assert!(concealed.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn rejects_wrong_frame_size() {
        let mut encoder = OpusEncoder::new(DEFAULT_BITRATE).unwrap();
        assert!(encoder.encode(&[0.0; 10]).is_err());
    }

    #[test]
    fn applies_runtime_resilience_parameters() {
        let mut encoder = OpusEncoder::new(DEFAULT_BITRATE).unwrap();
        encoder
            .apply_adaptation(AudioAdaptationDecision {
                target_bitrate_bps: 24_000,
                fec_enabled: true,
                expected_packet_loss_percent: 20,
                dtx_enabled: true,
            })
            .unwrap();
        assert!(!encoder.encode(&[0.0; FRAME_SAMPLES]).unwrap().is_empty());
    }

    #[test]
    fn encoder_defaults_preserve_fullband_voice_quality() {
        let encoder = OpusEncoder::new(DEFAULT_BITRATE).unwrap();
        assert_eq!(encoder.inner.complexity().unwrap(), 10);
        assert!(encoder.inner.vbr().unwrap());
        assert!(!encoder.inner.vbr_constraint().unwrap());
        assert_eq!(encoder.inner.signal().unwrap(), Signal::Voice);
        assert_eq!(encoder.inner.max_bandwidth().unwrap(), Bandwidth::Fullband);
        assert_eq!(encoder.inner.lsb_depth().unwrap(), 24);
    }

    #[test]
    fn every_room_quality_level_carries_voice_band_audio() {
        const ROOM_BITRATES: [i32; 10] = [
            16_000, 24_000, 32_000, 40_000, 48_000, 56_000, 64_000, 72_000, 80_000, 96_000,
        ];
        for bitrate in ROOM_BITRATES {
            let mut encoder = OpusEncoder::new(bitrate).unwrap();
            let mut decoder = OpusDecoder::new().unwrap();
            let mut decoded_energy = 0.0_f32;
            for frame_index in 0..12 {
                let input: [f32; FRAME_SAMPLES] = std::array::from_fn(|sample| {
                    let time = (frame_index * FRAME_SAMPLES + sample) as f32 / SAMPLE_RATE as f32;
                    0.12 * (220.0 * TAU * time).sin()
                        + 0.06 * (1_100.0 * TAU * time).sin()
                        + 0.03 * (3_200.0 * TAU * time).sin()
                });
                let payload = encoder.encode(&input).unwrap().to_vec();
                assert!(!payload.is_empty(), "empty Opus packet at {bitrate} bps");
                assert!(payload.len() < MAX_OPUS_PAYLOAD);
                let mut output = [0.0; FRAME_SAMPLES];
                assert_eq!(
                    decoder.decode(Some(&payload), &mut output).unwrap(),
                    FRAME_SAMPLES
                );
                assert!(output.iter().all(|sample| sample.is_finite()));
                if frame_index > 0 {
                    decoded_energy += output.iter().map(|sample| sample * sample).sum::<f32>();
                }
            }
            assert!(
                decoded_energy > 1.0,
                "voice-band energy disappeared at {bitrate} bps"
            );
        }
    }
}
