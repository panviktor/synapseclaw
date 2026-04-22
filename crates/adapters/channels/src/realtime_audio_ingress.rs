use crate::traits::ChannelMessage;
use parking_lot::RwLock as ParkingRwLock;
use std::collections::HashMap;
use std::sync::OnceLock;
use synapse_domain::config::schema::TranscriptionConfig;
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct RealtimeAudioIngressContext {
    pub tx: mpsc::Sender<ChannelMessage>,
    pub transcription: Option<TranscriptionConfig>,
}

pub fn register_realtime_audio_ingress(
    channel: &str,
    tx: mpsc::Sender<ChannelMessage>,
    transcription: Option<TranscriptionConfig>,
) {
    realtime_audio_ingress_slot().write().insert(
        channel.trim().to_ascii_lowercase(),
        RealtimeAudioIngressContext { tx, transcription },
    );
}

pub fn get_realtime_audio_ingress(channel: &str) -> Option<RealtimeAudioIngressContext> {
    realtime_audio_ingress_slot()
        .read()
        .get(&channel.trim().to_ascii_lowercase())
        .cloned()
}

fn realtime_audio_ingress_slot(
) -> &'static ParkingRwLock<HashMap<String, RealtimeAudioIngressContext>> {
    static SLOT: OnceLock<ParkingRwLock<HashMap<String, RealtimeAudioIngressContext>>> =
        OnceLock::new();
    SLOT.get_or_init(|| ParkingRwLock::new(HashMap::new()))
}

pub fn pcm16_wav_bytes(sample_rate: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
    let bytes_per_sample = 2u16;
    let data_bytes = samples.len().saturating_mul(bytes_per_sample as usize);
    let riff_chunk_size = 36u32.saturating_add(data_bytes as u32);
    let byte_rate = sample_rate
        .saturating_mul(channels as u32)
        .saturating_mul(bytes_per_sample as u32);
    let block_align = channels.saturating_mul(bytes_per_sample);

    let mut out = Vec::with_capacity(44 + data_bytes);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_chunk_size.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_bytes as u32).to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

#[derive(Debug, Clone)]
pub struct RealtimePcm16TurnChunker {
    sample_rate: u32,
    num_channels: u32,
    speech_start_samples: usize,
    min_speech_samples: usize,
    max_segment_samples: usize,
    trailing_silence_samples: usize,
    peak_threshold: i16,
    rms_threshold: i16,
    buffer: Vec<i16>,
    pending_voice: Vec<i16>,
    voiced_samples: usize,
    pending_voice_samples: usize,
    pending_silence_samples: usize,
    active: bool,
}

impl RealtimePcm16TurnChunker {
    pub fn new(sample_rate: u32, num_channels: u32) -> Self {
        let safe_rate = sample_rate.max(1);
        let safe_channels = num_channels.max(1);
        Self {
            sample_rate: safe_rate,
            num_channels: safe_channels,
            speech_start_samples: ((safe_rate as usize) * 180) / 1000,
            min_speech_samples: ((safe_rate as usize) * 450) / 1000,
            max_segment_samples: ((safe_rate as usize) * 6000) / 1000,
            trailing_silence_samples: ((safe_rate as usize) * 750) / 1000,
            peak_threshold: 900,
            rms_threshold: 140,
            buffer: Vec::new(),
            pending_voice: Vec::new(),
            voiced_samples: 0,
            pending_voice_samples: 0,
            pending_silence_samples: 0,
            active: false,
        }
    }

    pub fn push_frame(&mut self, data: &[i16]) -> Vec<Vec<i16>> {
        let mut flushed = Vec::new();
        let channels = self.num_channels as usize;
        if channels == 0 || data.is_empty() {
            return flushed;
        }

        let frame_samples = data.len() / channels;
        if frame_samples == 0 {
            return flushed;
        }

        let voiced = self.frame_is_voiced(data);

        if self.active {
            if voiced {
                self.buffer.extend_from_slice(data);
                self.voiced_samples = self.voiced_samples.saturating_add(frame_samples);
                self.pending_silence_samples = 0;
            } else {
                self.buffer.extend_from_slice(data);
                self.pending_silence_samples =
                    self.pending_silence_samples.saturating_add(frame_samples);
            }
        } else if voiced {
            self.pending_voice.extend_from_slice(data);
            self.pending_voice_samples =
                self.pending_voice_samples.saturating_add(frame_samples);
            if self.pending_voice_samples >= self.speech_start_samples {
                self.active = true;
                self.buffer.append(&mut self.pending_voice);
                self.voiced_samples = self.pending_voice_samples;
                self.pending_voice_samples = 0;
                self.pending_silence_samples = 0;
            }
        } else {
            self.pending_voice.clear();
            self.pending_voice_samples = 0;
        }

        if self.active
            && (self.voiced_samples >= self.max_segment_samples
                || (self.voiced_samples >= self.min_speech_samples
                    && self.pending_silence_samples >= self.trailing_silence_samples))
        {
            if let Some(segment) = self.flush_current_segment() {
                flushed.push(segment);
            }
        }

        flushed
    }

    pub fn finish(&mut self) -> Option<Vec<i16>> {
        self.flush_current_segment()
    }

    pub fn reset(&mut self) {
        self.buffer.clear();
        self.pending_voice.clear();
        self.voiced_samples = 0;
        self.pending_voice_samples = 0;
        self.pending_silence_samples = 0;
        self.active = false;
    }

    fn flush_current_segment(&mut self) -> Option<Vec<i16>> {
        let channels = self.num_channels as usize;
        let trimmed_len = self
            .buffer
            .len()
            .saturating_sub(self.pending_silence_samples.saturating_mul(channels));
        let segment = if self.voiced_samples >= self.min_speech_samples && trimmed_len > 0 {
            Some(self.buffer[..trimmed_len].to_vec())
        } else {
            None
        };

        self.reset();
        segment
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn num_channels(&self) -> u32 {
        self.num_channels
    }

    fn frame_is_voiced(&self, data: &[i16]) -> bool {
        if data.is_empty() {
            return false;
        }

        let peak = data
            .iter()
            .map(|sample| sample.unsigned_abs())
            .max()
            .unwrap_or(0);
        if peak < self.peak_threshold as u16 {
            return false;
        }

        let energy: f64 = data
            .iter()
            .map(|sample| {
                let value = *sample as f64;
                value * value
            })
            .sum();
        let rms = (energy / data.len() as f64).sqrt();
        rms >= self.rms_threshold as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_bytes_have_basic_header() {
        let wav = pcm16_wav_bytes(16_000, 1, &[0, 100, -100, 0]);
        assert!(wav.starts_with(b"RIFF"));
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");
    }

    #[test]
    fn chunker_flushes_after_voice_then_silence() {
        let mut chunker = RealtimePcm16TurnChunker::new(16_000, 1);
        let voiced = vec![2_000i16; 320];
        let silence = vec![0i16; 320];

        let mut segments = Vec::new();
        for _ in 0..40 {
            segments.extend(chunker.push_frame(&voiced));
        }
        assert!(segments.is_empty());

        for _ in 0..60 {
            segments.extend(chunker.push_frame(&silence));
            if !segments.is_empty() {
                break;
            }
        }

        assert_eq!(segments.len(), 1);
        assert!(!segments[0].is_empty());
    }

    #[test]
    fn chunker_ignores_short_noise() {
        let mut chunker = RealtimePcm16TurnChunker::new(16_000, 1);
        let voiced = vec![1_500i16; 320];
        for _ in 0..5 {
            assert!(chunker.push_frame(&voiced).is_empty());
        }
        assert!(chunker.finish().is_none());
    }

    #[test]
    fn chunker_requires_sustained_voiced_start() {
        let mut chunker = RealtimePcm16TurnChunker::new(16_000, 1);
        let short_burst = vec![2_000i16; 80];
        let silence = vec![0i16; 320];

        assert!(chunker.push_frame(&short_burst).is_empty());
        assert!(chunker.push_frame(&silence).is_empty());
        assert!(chunker.finish().is_none());
    }
}
