use songbird::{
    events::context_data::VoiceTick,
    model::payload::Speaking,
    Event, EventContext, EventHandler,
};
use std::{
    collections::HashMap,
    sync::Arc,
};
use tokio::sync::Mutex;
use tracing::{info, warn};

/// Sample rate for audio (48kHz is Discord's native rate)
pub const SAMPLE_RATE: u32 = 48000;
/// Number of channels (stereo)
pub const CHANNELS: u16 = 2;
pub const FRAME_DURATION_IN_MS: f32 = 20.0;
/// Samples per 20ms frame at 48kHz stereo (48000 * 0.020 * 2 = 1920)
pub const SAMPLES_PER_FRAME: usize = (SAMPLE_RATE as f32 * CHANNELS as f32 * FRAME_DURATION_IN_MS / 1000.0) as usize;

/// Maps SSRC (audio stream ID) to Discord User ID
pub type SsrcUserMap = Arc<Mutex<HashMap<u32, u64>>>;
/// Maps User ID to their recorded PCM audio buffer
pub type UserAudioBuffer = Arc<Mutex<HashMap<u64, Vec<i16>>>>;

/// Receiver handles voice events from Songbird
pub struct Receiver {
    /// Maps SSRC to User ID (populated when users start speaking)
    ssrc_map: SsrcUserMap,
    /// Stores recorded audio per user
    audio_buffers: UserAudioBuffer,
    /// Whether recording is currently active
    recording_active: Arc<Mutex<bool>>,
}

impl Receiver {
    pub fn new(
        ssrc_map: SsrcUserMap,
        audio_buffers: UserAudioBuffer,
        recording_active: Arc<Mutex<bool>>,
    ) -> Self {
        Self {
            ssrc_map,
            audio_buffers,
            recording_active,
        }
    }
}

#[async_trait::async_trait]
impl EventHandler for Receiver {
    async fn act(&self, ctx: &EventContext<'_>) -> Option<Event> {
        match ctx {
            EventContext::SpeakingStateUpdate(Speaking {
                speaking: _,
                ssrc,
                user_id,
                ..
            }) => {
                // Map SSRC to User ID when a user starts/stops speaking
                if let Some(user_id) = user_id {
                    let mut map = self.ssrc_map.lock().await;
                    map.insert(*ssrc, user_id.0);
                    info!("Mapped SSRC {} to User ID {}", ssrc, user_id.0);
                }
            }
            EventContext::VoiceTick(VoiceTick {
                speaking,
                silent: _,
                ..
            }) => {
                // Check if recording is active
                if !*self.recording_active.lock().await {
                    return None;
                }

                let ssrc_map = self.ssrc_map.lock().await;
                let mut audio_buffers = self.audio_buffers.lock().await;

                // Process each speaker in this tick
                for (ssrc, voice_data) in speaking {
                    // Look up the user ID for this SSRC
                    if let Some(&user_id) = ssrc_map.get(ssrc) {
                        // Get or create the audio buffer for this user
                        let buffer = audio_buffers.entry(user_id).or_insert_with(Vec::new);

                        // Check if we have decoded audio
                        if let Some(decoded) = &voice_data.decoded_voice {
                            // Append the decoded PCM samples
                            buffer.extend_from_slice(decoded);
                        } else {
                            // No decoded audio available, write silence
                            buffer.extend(std::iter::repeat(0i16).take(SAMPLES_PER_FRAME));
                            warn!(
                                "No decoded audio for SSRC {}, writing silence frame",
                                ssrc
                            );
                        }
                    }
                }

                // For users who are known but not speaking in this tick,
                // we write silence to keep audio in sync
                for &user_id in ssrc_map.values() {
                    let is_speaking = speaking.values().any(|vd| {
                        ssrc_map
                            .iter()
                            .find(|(_, &uid)| uid == user_id)
                            .map(|(ssrc, _)| speaking.contains_key(ssrc))
                            .unwrap_or(false)
                    });

                    if !is_speaking {
                        if let Some(buffer) = audio_buffers.get_mut(&user_id) {
                            // Only add silence if user has previously spoken
                            // (i.e., has existing audio data)
                            if !buffer.is_empty() {
                                buffer.extend(std::iter::repeat(0i16).take(SAMPLES_PER_FRAME));
                            }
                        }
                    }
                }
            }
            _ => {}
        }

        None
    }
}

/// Mixes multiple user audio buffers into a single stereo buffer
pub fn mix_audio_buffers(buffers: &HashMap<u64, Vec<i16>>) -> Vec<i16> {
    if buffers.is_empty() {
        return Vec::new();
    }

    // Find the maximum length among all buffers
    let max_len = buffers.values().map(|b| b.len()).max().unwrap_or(0);
    if max_len == 0 {
        return Vec::new();
    }

    let mut mixed = vec![0i32; max_len];

    // Sum all buffers
    for buffer in buffers.values() {
        for (i, &sample) in buffer.iter().enumerate() {
            mixed[i] += sample as i32;
        }
    }

    // Normalize and clamp to i16 range
    let num_sources = buffers.len() as i32;
    mixed
        .into_iter()
        .map(|sample| {
            let normalized = sample / num_sources;
            normalized.clamp(i16::MIN as i32, i16::MAX as i32) as i16
        })
        .collect()
}

/// Saves PCM audio data to a WAV file
pub fn save_to_wav(
    pcm_data: &[i16],
    file_path: &str,
    sample_rate: u32,
    channels: u16,
) -> Result<(), hound::Error> {
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut writer = hound::WavWriter::create(file_path, spec)?;
    for &sample in pcm_data {
        writer.write_sample(sample)?;
    }
    writer.finalize()?;

    info!("Saved WAV file to: {}", file_path);
    Ok(())
}

