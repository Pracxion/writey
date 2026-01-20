use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use thiserror::Error;
use tracing::info;

/// Original sample rate from Discord voice (Opus decoded)
const SOURCE_SAMPLE_RATE: u32 = 48000;
/// Whisper's required sample rate
pub const WHISPER_SAMPLE_RATE: u32 = 16000;
/// Samples per frame at 48kHz (20ms frames)
const SAMPLES_PER_FRAME: usize = 960;

/// Minimum silence duration to split chunks (in seconds)
pub const MIN_SILENCE_DURATION_SECS: f32 = 2.0;
/// Silence threshold - samples below this (absolute) are considered silence
/// This is normalized, so 0.01 = about -40dB
const SILENCE_THRESHOLD: f32 = 0.01;
/// Window size for silence detection (in samples at 16kHz)
const SILENCE_WINDOW_SIZE: usize = 1600; // 100ms windows

#[derive(Error, Debug)]
pub enum TranscribeError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Session directory not found: {0}")]
    SessionNotFound(PathBuf),
    #[error("Users directory not found in session")]
    UsersNotFound,
    #[error("User directory not found: {0}")]
    UserNotFound(String),
    #[error("No audio data found for user")]
    NoAudioData,
    #[error("Failed to parse log file: {0}")]
    ParseError(String),
    #[error("SSRC map not found or invalid")]
    SsrcMapError,
}

/// Audio data prepared for Whisper transcription
/// 
/// Multiple SSRCs belonging to the same user are merged into one PreparedAudio
#[derive(Debug, Clone)]
pub struct PreparedAudio {
    /// User ID (Discord user ID from ssrc_map)
    pub user_id: u64,
    /// All SSRC identifiers that were merged for this user
    pub ssrcs: Vec<u32>,
    /// Audio samples at 16kHz (Whisper format), normalized to [-1.0, 1.0]
    pub samples_16khz: Vec<f32>,
    /// Duration in seconds
    pub duration_secs: f32,
    /// First tick index (for timing reference)
    pub first_tick: u64,
    /// Last tick index
    pub last_tick: u64,
}

/// A single audio chunk split on silence boundaries
#[derive(Debug, Clone)]
pub struct AudioChunk {
    /// Chunk index (0-based)
    pub index: usize,
    /// Audio samples at 16kHz
    pub samples: Vec<f32>,
    /// Start time offset in seconds (relative to user's first audio)
    pub start_time_secs: f32,
    /// End time offset in seconds
    pub end_time_secs: f32,
    /// Duration in seconds
    pub duration_secs: f32,
}

impl AudioChunk {
    /// Get the audio as WAV bytes
    pub fn as_wav_bytes(&self) -> Vec<u8> {
        samples_to_wav_bytes(&self.samples)
    }
}

/// Convert samples to WAV bytes
fn samples_to_wav_bytes(samples: &[f32]) -> Vec<u8> {
    let mut buffer = Vec::new();
    
    let data_size = (samples.len() * 2) as u32;
    let file_size = 36 + data_size;
    
    // RIFF header
    buffer.extend_from_slice(b"RIFF");
    buffer.extend_from_slice(&file_size.to_le_bytes());
    buffer.extend_from_slice(b"WAVE");
    
    // fmt chunk
    buffer.extend_from_slice(b"fmt ");
    buffer.extend_from_slice(&16u32.to_le_bytes());
    buffer.extend_from_slice(&1u16.to_le_bytes()); // PCM
    buffer.extend_from_slice(&1u16.to_le_bytes()); // mono
    buffer.extend_from_slice(&WHISPER_SAMPLE_RATE.to_le_bytes());
    buffer.extend_from_slice(&(WHISPER_SAMPLE_RATE * 2).to_le_bytes());
    buffer.extend_from_slice(&2u16.to_le_bytes());
    buffer.extend_from_slice(&16u16.to_le_bytes());
    
    // data chunk
    buffer.extend_from_slice(b"data");
    buffer.extend_from_slice(&data_size.to_le_bytes());
    
    for sample in samples {
        let clamped = sample.clamp(-1.0, 1.0);
        let i16_sample = (clamped * 32767.0) as i16;
        buffer.extend_from_slice(&i16_sample.to_le_bytes());
    }
    
    buffer
}

/// Check if a window of samples is silence
fn is_silence_window(samples: &[f32]) -> bool {
    if samples.is_empty() {
        return true;
    }
    
    // Calculate RMS (root mean square) for the window
    let sum_squares: f32 = samples.iter().map(|s| s * s).sum();
    let rms = (sum_squares / samples.len() as f32).sqrt();
    
    rms < SILENCE_THRESHOLD
}

/// Find silence regions in the audio
/// Returns a list of (start_sample, end_sample) for each silence region >= min_duration
fn find_silence_regions(samples: &[f32], min_silence_samples: usize) -> Vec<(usize, usize)> {
    let mut regions = Vec::new();
    let mut in_silence = false;
    let mut silence_start = 0;
    
    let mut i = 0;
    while i < samples.len() {
        let window_end = (i + SILENCE_WINDOW_SIZE).min(samples.len());
        let window = &samples[i..window_end];
        let is_silent = is_silence_window(window);
        
        if is_silent && !in_silence {
            // Start of silence region
            in_silence = true;
            silence_start = i;
        } else if !is_silent && in_silence {
            // End of silence region
            in_silence = false;
            let silence_len = i - silence_start;
            if silence_len >= min_silence_samples {
                regions.push((silence_start, i));
            }
        }
        
        i += SILENCE_WINDOW_SIZE;
    }
    
    // Handle case where audio ends in silence
    if in_silence {
        let silence_len = samples.len() - silence_start;
        if silence_len >= min_silence_samples {
            regions.push((silence_start, samples.len()));
        }
    }
    
    regions
}

/// Split samples into chunks based on silence regions
fn split_on_silence(samples: &[f32], min_silence_secs: f32) -> Vec<AudioChunk> {
    if samples.is_empty() {
        return Vec::new();
    }
    
    let min_silence_samples = (min_silence_secs * WHISPER_SAMPLE_RATE as f32) as usize;
    let silence_regions = find_silence_regions(samples, min_silence_samples);
    
    if silence_regions.is_empty() {
        // No silence gaps found, return whole audio as single chunk
        return vec![AudioChunk {
            index: 0,
            samples: samples.to_vec(),
            start_time_secs: 0.0,
            end_time_secs: samples.len() as f32 / WHISPER_SAMPLE_RATE as f32,
            duration_secs: samples.len() as f32 / WHISPER_SAMPLE_RATE as f32,
        }];
    }
    
    let mut chunks = Vec::new();
    let mut chunk_start = 0;
    
    for (silence_start, silence_end) in &silence_regions {
        // Create chunk from chunk_start to middle of silence region
        let split_point = silence_start + (silence_end - silence_start) / 2;
        
        if split_point > chunk_start {
            let chunk_samples = samples[chunk_start..split_point].to_vec();
            
            // Skip chunks that are too short (< 0.5 seconds) or all silence
            if chunk_samples.len() >= (WHISPER_SAMPLE_RATE / 2) as usize 
                && !is_silence_window(&chunk_samples) 
            {
                let start_time = chunk_start as f32 / WHISPER_SAMPLE_RATE as f32;
                let end_time = split_point as f32 / WHISPER_SAMPLE_RATE as f32;
                
                chunks.push(AudioChunk {
                    index: chunks.len(),
                    samples: chunk_samples,
                    start_time_secs: start_time,
                    end_time_secs: end_time,
                    duration_secs: end_time - start_time,
                });
            }
        }
        
        chunk_start = split_point;
    }
    
    // Handle remaining audio after last silence
    if chunk_start < samples.len() {
        let chunk_samples = samples[chunk_start..].to_vec();
        
        if chunk_samples.len() >= (WHISPER_SAMPLE_RATE / 2) as usize
            && !is_silence_window(&chunk_samples)
        {
            let start_time = chunk_start as f32 / WHISPER_SAMPLE_RATE as f32;
            let end_time = samples.len() as f32 / WHISPER_SAMPLE_RATE as f32;
            
            chunks.push(AudioChunk {
                index: chunks.len(),
                samples: chunk_samples,
                start_time_secs: start_time,
                end_time_secs: end_time,
                duration_secs: end_time - start_time,
            });
        }
    }
    
    // Re-index chunks
    for (i, chunk) in chunks.iter_mut().enumerate() {
        chunk.index = i;
    }
    
    chunks
}

impl PreparedAudio {
    /// Get the audio as WAV bytes (for file writing or API calls)
    pub fn as_wav_bytes(&self) -> Vec<u8> {
        samples_to_wav_bytes(&self.samples_16khz)
    }
    
    /// Split the audio into chunks based on silence gaps
    /// 
    /// Chunks are split when there is silence for at least `min_silence_secs`.
    /// Each chunk contains timing metadata for later timestamp reconstruction.
    pub fn split_on_silence(&self, min_silence_secs: f32) -> Vec<AudioChunk> {
        info!(
            "Splitting {:.1}s of audio on silence gaps >= {:.1}s",
            self.duration_secs, min_silence_secs
        );
        
        let chunks = split_on_silence(&self.samples_16khz, min_silence_secs);
        
        info!(
            "Split into {} chunks",
            chunks.len()
        );
        
        for chunk in &chunks {
            info!(
                "  Chunk {}: {:.2}s - {:.2}s ({:.2}s)",
                chunk.index, chunk.start_time_secs, chunk.end_time_secs, chunk.duration_secs
            );
        }
        
        chunks
    }
    
    /// Split using the default silence duration (2 seconds)
    pub fn split_on_silence_default(&self) -> Vec<AudioChunk> {
        self.split_on_silence(MIN_SILENCE_DURATION_SECS)
    }
}

#[derive(Debug)]
struct AudioFrame {
    tick_index: u64,
    samples: Vec<i16>,
}

/// Load and parse a single log file
fn parse_log_file(path: &Path) -> Result<Vec<AudioFrame>, TranscribeError> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut frames = Vec::new();

    for (line_num, line) in reader.lines().enumerate() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let mut parts = line.splitn(2, ' ');
        let tick_str = parts.next().ok_or_else(|| {
            TranscribeError::ParseError(format!("Missing tick index at line {}", line_num + 1))
        })?;
        let samples_str = parts.next().ok_or_else(|| {
            TranscribeError::ParseError(format!("Missing samples at line {}", line_num + 1))
        })?;

        let tick_index: u64 = tick_str.parse().map_err(|_| {
            TranscribeError::ParseError(format!("Invalid tick index at line {}", line_num + 1))
        })?;

        let samples: Vec<i16> = samples_str
            .split(',')
            .map(|s| s.trim().parse::<i16>())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| {
                TranscribeError::ParseError(format!("Invalid sample data at line {}", line_num + 1))
            })?;

        frames.push(AudioFrame { tick_index, samples });
    }

    Ok(frames)
}

/// Load all chunks for a user directory and return ordered frames
fn load_user_chunks(user_dir: &Path) -> Result<BTreeMap<u64, Vec<i16>>, TranscribeError> {
    let mut all_frames: BTreeMap<u64, Vec<i16>> = BTreeMap::new();

    // Find all chunk-*.log files
    let mut chunk_files: Vec<PathBuf> = fs::read_dir(user_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("chunk-") && n.ends_with(".log"))
                .unwrap_or(false)
        })
        .collect();

    // Sort by chunk number
    chunk_files.sort_by(|a, b| {
        let get_num = |p: &PathBuf| -> u32 {
            p.file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| s.strip_prefix("chunk-"))
                .and_then(|s| s.parse().ok())
                .unwrap_or(0)
        };
        get_num(a).cmp(&get_num(b))
    });

    for chunk_path in chunk_files {
        let frames = parse_log_file(&chunk_path)?;
        for frame in frames {
            all_frames.insert(frame.tick_index, frame.samples);
        }
    }

    Ok(all_frames)
}

/// Downsample from 48kHz to 16kHz using averaging
fn downsample_48k_to_16k(samples_48k: &[i16]) -> Vec<f32> {
    let ratio = SOURCE_SAMPLE_RATE as usize / WHISPER_SAMPLE_RATE as usize;
    
    samples_48k
        .chunks(ratio)
        .map(|chunk| {
            let sum: i32 = chunk.iter().map(|&s| s as i32).sum();
            let avg = sum / chunk.len() as i32;
            avg as f32 / 32768.0
        })
        .collect()
}

/// Reconstruct continuous audio from frame map, filling gaps with silence
fn reconstruct_audio(frames: &BTreeMap<u64, Vec<i16>>) -> (Vec<i16>, u64, u64) {
    if frames.is_empty() {
        return (Vec::new(), 0, 0);
    }

    let first_tick = *frames.keys().next().unwrap();
    let last_tick = *frames.keys().next_back().unwrap();
    
    let silence = vec![0i16; SAMPLES_PER_FRAME];
    let mut audio = Vec::new();

    for tick in first_tick..=last_tick {
        let samples = frames.get(&tick).unwrap_or(&silence);
        audio.extend_from_slice(samples);
    }

    (audio, first_tick, last_tick)
}

/// Load SSRC to user ID mapping from session directory
pub fn load_ssrc_map(session_dir: &Path) -> Result<HashMap<u32, u64>, TranscribeError> {
    let ssrc_map_path = session_dir.join("ssrc_map.json");
    if !ssrc_map_path.exists() {
        return Err(TranscribeError::SsrcMapError);
    }
    
    let file = File::open(&ssrc_map_path)?;
    let reader = BufReader::new(file);
    let map: HashMap<String, u64> = serde_json::from_reader(reader)
        .map_err(|_| TranscribeError::SsrcMapError)?;
    
    // Convert string keys to u32
    let converted: HashMap<u32, u64> = map
        .into_iter()
        .filter_map(|(k, v)| k.parse::<u32>().ok().map(|ssrc| (ssrc, v)))
        .collect();
    
    Ok(converted)
}

/// Invert the SSRC map: group SSRCs by user ID
/// 
/// Since multiple SSRCs can map to the same user ID, this creates
/// a map from user_id -> Vec<ssrc>
pub fn group_ssrcs_by_user(ssrc_map: &HashMap<u32, u64>) -> HashMap<u64, Vec<u32>> {
    let mut user_ssrcs: HashMap<u64, Vec<u32>> = HashMap::new();
    
    for (&ssrc, &user_id) in ssrc_map {
        user_ssrcs.entry(user_id).or_default().push(ssrc);
    }
    
    // Sort SSRCs for consistent ordering
    for ssrcs in user_ssrcs.values_mut() {
        ssrcs.sort();
    }
    
    user_ssrcs
}

/// Merge multiple frame maps into one, combining overlapping ticks
fn merge_frame_maps(maps: Vec<BTreeMap<u64, Vec<i16>>>) -> BTreeMap<u64, Vec<i16>> {
    if maps.is_empty() {
        return BTreeMap::new();
    }
    if maps.len() == 1 {
        return maps.into_iter().next().unwrap();
    }

    let mut merged: BTreeMap<u64, Vec<i16>> = BTreeMap::new();

    for map in maps {
        for (tick, samples) in map {
            merged
                .entry(tick)
                .and_modify(|existing| {
                    // Mix the samples together
                    for (i, &sample) in samples.iter().enumerate() {
                        if i < existing.len() {
                            let mixed = existing[i] as i32 + sample as i32;
                            existing[i] = mixed.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
                        }
                    }
                })
                .or_insert(samples);
        }
    }

    merged
}

/// Load and prepare a single user's audio for Whisper transcription
/// 
/// This function merges audio from all SSRCs belonging to the same user.
/// 
/// # Arguments
/// * `session_dir` - Path to the recording session directory
/// * `user_id` - The Discord user ID
/// * `ssrcs` - All SSRCs belonging to this user
/// 
/// # Returns
/// * `PreparedAudio` containing 16kHz audio ready for Whisper
pub fn load_user_audio_for_transcription(
    session_dir: &Path,
    user_id: u64,
    ssrcs: &[u32],
) -> Result<PreparedAudio, TranscribeError> {
    if !session_dir.exists() {
        return Err(TranscribeError::SessionNotFound(session_dir.to_path_buf()));
    }

    let users_dir = session_dir.join("users");
    if !users_dir.exists() {
        return Err(TranscribeError::UsersNotFound);
    }

    info!("Loading audio for user {} with SSRCs {:?}", user_id, ssrcs);

    // Load frames from all SSRCs
    let mut all_frame_maps = Vec::new();
    
    for &ssrc in ssrcs {
        let user_dir = users_dir.join(ssrc.to_string());
        if !user_dir.exists() {
            tracing::warn!("SSRC directory not found: {}", ssrc);
            continue;
        }

        match load_user_chunks(&user_dir) {
            Ok(frames) if !frames.is_empty() => {
                info!("Loaded {} frames from SSRC {}", frames.len(), ssrc);
                all_frame_maps.push(frames);
            }
            Ok(_) => {
                tracing::warn!("No frames found for SSRC {}", ssrc);
            }
            Err(e) => {
                tracing::warn!("Failed to load SSRC {}: {}", ssrc, e);
            }
        }
    }

    if all_frame_maps.is_empty() {
        return Err(TranscribeError::NoAudioData);
    }

    // Merge all frame maps
    let merged_frames = merge_frame_maps(all_frame_maps);
    
    // Reconstruct continuous audio
    let (audio_48k, first_tick, last_tick) = reconstruct_audio(&merged_frames);
    
    info!(
        "Merged {} samples at 48kHz ({:.1}s), ticks {}-{}",
        audio_48k.len(),
        audio_48k.len() as f32 / SOURCE_SAMPLE_RATE as f32,
        first_tick,
        last_tick
    );

    // Downsample to 16kHz for Whisper
    let samples_16khz = downsample_48k_to_16k(&audio_48k);
    let duration_secs = samples_16khz.len() as f32 / WHISPER_SAMPLE_RATE as f32;

    info!(
        "Downsampled to {} samples at 16kHz ({:.1}s)",
        samples_16khz.len(),
        duration_secs
    );

    Ok(PreparedAudio {
        user_id,
        ssrcs: ssrcs.to_vec(),
        samples_16khz,
        duration_secs,
        first_tick,
        last_tick,
    })
}

/// Prepare all users in a session for transcription
/// 
/// This function:
/// 1. Loads the SSRC map
/// 2. Groups SSRCs by user ID (handling multiple SSRCs per user)
/// 3. Merges audio from all SSRCs for each user
/// 
/// # Arguments
/// * `session_dir` - Path to the recording session directory
/// 
/// # Returns
/// * Vector of `PreparedAudio` for each unique user in the session
pub fn prepare_session_for_transcription(
    session_dir: &Path,
) -> Result<Vec<PreparedAudio>, TranscribeError> {
    if !session_dir.exists() {
        return Err(TranscribeError::SessionNotFound(session_dir.to_path_buf()));
    }

    let users_dir = session_dir.join("users");
    if !users_dir.exists() {
        return Err(TranscribeError::UsersNotFound);
    }

    // Load SSRC map and group by user
    let ssrc_map = load_ssrc_map(session_dir)?;
    let user_ssrcs = group_ssrcs_by_user(&ssrc_map);

    info!(
        "Found {} unique users from {} SSRCs",
        user_ssrcs.len(),
        ssrc_map.len()
    );

    let mut prepared = Vec::new();

    for (user_id, ssrcs) in user_ssrcs {
        match load_user_audio_for_transcription(session_dir, user_id, &ssrcs) {
            Ok(audio) => {
                info!(
                    "Prepared user {} ({} SSRCs): {:.1}s of audio",
                    user_id,
                    ssrcs.len(),
                    audio.duration_secs
                );
                prepared.push(audio);
            }
            Err(e) => {
                tracing::warn!("Failed to prepare user {}: {}", user_id, e);
            }
        }
    }

    if prepared.is_empty() {
        return Err(TranscribeError::NoAudioData);
    }

    // Sort by first tick for chronological order
    prepared.sort_by_key(|a| a.first_tick);

    Ok(prepared)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_downsample_48k_to_16k() {
        let samples_48k: Vec<i16> = vec![100, 200, 300, 400, 500, 600, 700, 800, 900];
        let samples_16k = downsample_48k_to_16k(&samples_48k);
        
        assert_eq!(samples_16k.len(), 3);
        assert!((samples_16k[0] - (200.0 / 32768.0)).abs() < 0.001);
        assert!((samples_16k[1] - (500.0 / 32768.0)).abs() < 0.001);
        assert!((samples_16k[2] - (800.0 / 32768.0)).abs() < 0.001);
    }

    #[test]
    fn test_group_ssrcs_by_user() {
        let mut ssrc_map = HashMap::new();
        ssrc_map.insert(1000, 12345);
        ssrc_map.insert(1001, 12345); // Same user
        ssrc_map.insert(2000, 67890);
        
        let grouped = group_ssrcs_by_user(&ssrc_map);
        
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped.get(&12345).unwrap(), &vec![1000, 1001]);
        assert_eq!(grouped.get(&67890).unwrap(), &vec![2000]);
    }

    #[test]
    fn test_wav_bytes_header() {
        let audio = PreparedAudio {
            user_id: 12345,
            ssrcs: vec![1234],
            samples_16khz: vec![0.0, 0.5, -0.5],
            duration_secs: 0.0001875,
            first_tick: 0,
            last_tick: 0,
        };
        
        let wav = audio.as_wav_bytes();
        
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");
    }
}
