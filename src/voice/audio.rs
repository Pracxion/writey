use super::storage::AudioFrame;
use tracing::info;

#[derive(Debug, Clone, Copy)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
}

impl AudioFormat {
    pub const DISCORD_NATIVE: Self = Self {
        sample_rate: 48000,
        channels: 2,
    };

    pub const CAPTURE_MONO: Self = Self {
        sample_rate: 48000,
        channels: 1,
    };

    pub const TRANSCRIPTION: Self = Self {
        sample_rate: 16000,
        channels: 1,
    };

    pub fn samples_per_frame(&self) -> usize {
        (self.sample_rate as f32 * self.channels as f32 * 0.020) as usize
    }

    /// Samples per tick (mono, for one channel)
    pub fn samples_per_tick_mono(&self) -> usize {
        (self.sample_rate as f32 * 0.020) as usize
    }
}

/// Convert stereo PCM to mono by averaging channels
pub fn stereo_to_mono(stereo: &[i16]) -> Vec<i16> {
    stereo
        .chunks(2)
        .map(|chunk| {
            if chunk.len() == 2 {
                ((chunk[0] as i32 + chunk[1] as i32) / 2) as i16
            } else {
                chunk[0]
            }
        })
        .collect()
}

/// Downsample audio from source_rate to target_rate using linear interpolation
pub fn downsample(samples: &[i16], source_rate: u32, target_rate: u32) -> Vec<i16> {
    if source_rate == target_rate {
        return samples.to_vec();
    }

    let ratio = source_rate as f64 / target_rate as f64;
    let output_len = (samples.len() as f64 / ratio).ceil() as usize;
    let mut output = Vec::with_capacity(output_len);

    for i in 0..output_len {
        let src_pos = i as f64 * ratio;
        let src_idx = src_pos.floor() as usize;
        let frac = src_pos - src_idx as f64;

        let sample = if src_idx + 1 < samples.len() {
            // Linear interpolation
            let s0 = samples[src_idx] as f64;
            let s1 = samples[src_idx + 1] as f64;
            (s0 + (s1 - s0) * frac) as i16
        } else if src_idx < samples.len() {
            samples[src_idx]
        } else {
            0
        };

        output.push(sample);
    }

    output
}

/// Upsample audio from source_rate to target_rate using linear interpolation
pub fn upsample(samples: &[i16], source_rate: u32, target_rate: u32) -> Vec<i16> {
    if source_rate == target_rate {
        return samples.to_vec();
    }

    let ratio = target_rate as f64 / source_rate as f64;
    let output_len = (samples.len() as f64 * ratio).ceil() as usize;
    let mut output = Vec::with_capacity(output_len);

    for i in 0..output_len {
        let src_pos = i as f64 / ratio;
        let src_idx = src_pos.floor() as usize;
        let frac = src_pos - src_idx as f64;

        let sample = if src_idx + 1 < samples.len() {
            let s0 = samples[src_idx] as f64;
            let s1 = samples[src_idx + 1] as f64;
            (s0 + (s1 - s0) * frac) as i16
        } else if src_idx < samples.len() {
            samples[src_idx]
        } else {
            0
        };

        output.push(sample);
    }

    output
}

/// Resample audio to a different sample rate
pub fn resample(samples: &[i16], source_rate: u32, target_rate: u32) -> Vec<i16> {
    if source_rate == target_rate {
        samples.to_vec()
    } else if source_rate > target_rate {
        downsample(samples, source_rate, target_rate)
    } else {
        upsample(samples, source_rate, target_rate)
    }
}

/// Process audio for transcription: convert to mono 16kHz 16-bit
pub fn prepare_for_transcription(
    samples: &[i16],
    source_format: AudioFormat,
) -> Vec<i16> {
    let mut processed = samples.to_vec();

    // Convert to mono if stereo
    if source_format.channels == 2 {
        processed = stereo_to_mono(&processed);
    }

    // Resample to 16kHz
    if source_format.sample_rate != AudioFormat::TRANSCRIPTION.sample_rate {
        processed = resample(
            &processed,
            source_format.sample_rate,
            AudioFormat::TRANSCRIPTION.sample_rate,
        );
    }

    processed
}

/// Convert PCM i16 samples to f32 (normalized to -1.0 to 1.0)
pub fn pcm_to_float(samples: &[i16]) -> Vec<f32> {
    samples
        .iter()
        .map(|&s| s as f32 / i16::MAX as f32)
        .collect()
}

/// Convert f32 samples to PCM i16
pub fn float_to_pcm(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|&s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
        .collect()
}

/// Calculate RMS energy of samples
pub fn calculate_rms(samples: &[i16]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }

    let sum_squares: f64 = samples.iter().map(|&s| (s as f64).powi(2)).sum();
    (sum_squares / samples.len() as f64).sqrt()
}

/// Simple energy-based VAD (Voice Activity Detection)
pub fn detect_voice_activity(samples: &[i16], threshold_db: f64) -> bool {
    let rms = calculate_rms(samples);
    let db = 20.0 * (rms / i16::MAX as f64).log10();
    db > threshold_db
}

/// Trim silence from start and end of samples
pub fn trim_silence(samples: &[i16], threshold_db: f64, frame_size: usize) -> &[i16] {
    if samples.is_empty() {
        return samples;
    }

    // Find start
    let mut start = 0;
    for chunk in samples.chunks(frame_size) {
        if detect_voice_activity(chunk, threshold_db) {
            break;
        }
        start += chunk.len();
    }

    // Find end
    let mut end = samples.len();
    for chunk in samples.rchunks(frame_size) {
        if detect_voice_activity(chunk, threshold_db) {
            break;
        }
        end = end.saturating_sub(chunk.len());
    }

    if start >= end {
        return &samples[0..0];
    }

    &samples[start..end]
}

/// Normalize audio to a target peak level
pub fn normalize(samples: &[i16], target_peak: f32) -> Vec<i16> {
    if samples.is_empty() {
        return Vec::new();
    }

    let max_sample = samples.iter().map(|&s| s.abs()).max().unwrap_or(0) as f32;
    if max_sample == 0.0 {
        return samples.to_vec();
    }

    let target_max = target_peak * i16::MAX as f32;
    let gain = target_max / max_sample;

    samples
        .iter()
        .map(|&s| ((s as f32) * gain).clamp(i16::MIN as f32, i16::MAX as f32) as i16)
        .collect()
}

/// Mix multiple audio tracks together
pub fn mix_tracks(tracks: &[&[i16]]) -> Vec<i16> {
    if tracks.is_empty() {
        return Vec::new();
    }

    let max_len = tracks.iter().map(|t| t.len()).max().unwrap_or(0);
    let mut mixed = vec![0i32; max_len];

    for track in tracks {
        for (i, &sample) in track.iter().enumerate() {
            mixed[i] += sample as i32;
        }
    }

    let num_tracks = tracks.len() as i32;
    mixed
        .into_iter()
        .map(|s| (s / num_tracks).clamp(i16::MIN as i32, i16::MAX as i32) as i16)
        .collect()
}

/// Rebuild continuous PCM from sparse frames
pub fn rebuild_pcm_from_frames(
    frames: &[AudioFrame],
    samples_per_tick: usize,
) -> Vec<i16> {
    if frames.is_empty() {
        return Vec::new();
    }

    let first_tick = frames.first().unwrap().tick_index;
    let last_tick = frames.last().unwrap().tick_index;
    let total_ticks = (last_tick - first_tick + 1) as usize;

    let mut pcm = vec![0i16; total_ticks * samples_per_tick];

    for frame in frames {
        let relative_tick = (frame.tick_index - first_tick) as usize;
        let start_idx = relative_tick * samples_per_tick;
        let end_idx = (start_idx + frame.samples.len()).min(pcm.len());

        let copy_len = end_idx - start_idx;
        pcm[start_idx..end_idx].copy_from_slice(&frame.samples[..copy_len]);
    }

    pcm
}

/// Save PCM data to WAV file
pub fn save_wav(
    samples: &[i16],
    path: &str,
    sample_rate: u32,
    channels: u16,
) -> Result<(), hound::Error> {
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut writer = hound::WavWriter::create(path, spec)?;
    for &sample in samples {
        writer.write_sample(sample)?;
    }
    writer.finalize()?;

    info!("Saved WAV: {} ({} samples, {}Hz, {}ch)", 
        path, samples.len(), sample_rate, channels);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stereo_to_mono() {
        let stereo = vec![100i16, 200, 300, 400, 500, 600];
        let mono = stereo_to_mono(&stereo);
        assert_eq!(mono, vec![150, 350, 550]);
    }

    #[test]
    fn test_downsample() {
        // 48kHz to 16kHz = 1/3
        let samples: Vec<i16> = (0..48).collect();
        let downsampled = downsample(&samples, 48000, 16000);
        assert_eq!(downsampled.len(), 16);
    }

    #[test]
    fn test_calculate_rms() {
        let silence = vec![0i16; 100];
        assert_eq!(calculate_rms(&silence), 0.0);

        let signal = vec![100i16; 100];
        assert!((calculate_rms(&signal) - 100.0).abs() < 0.1);
    }
}

