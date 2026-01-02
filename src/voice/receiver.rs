use super::audio::{stereo_to_mono, AudioFormat};
use super::storage::{AudioFrame, SessionStorage};
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
use tracing::{info, warn, debug};

/// Sample rate for audio (48kHz is Discord's native rate)
pub const SAMPLE_RATE: u32 = 48000;
/// Number of channels for capture (mono for speech)
pub const CHANNELS: u16 = 1;
/// Frame duration in milliseconds
pub const FRAME_DURATION_MS: f32 = 20.0;
/// Samples per 20ms frame at 48kHz mono
pub const SAMPLES_PER_FRAME: usize = (SAMPLE_RATE as f32 * FRAME_DURATION_MS / 1000.0) as usize;

pub type SsrcUserMap = Arc<Mutex<HashMap<u32, u64>>>;

pub struct RecordingState {
    pub active: bool,
    pub tick_index: u64,
    pub ssrc_map: HashMap<u32, u64>,
    pub storage: Option<SessionStorage>,
}

impl RecordingState {
    pub fn new() -> Self {
        Self {
            active: false,
            tick_index: 0,
            ssrc_map: HashMap::new(),
            storage: None,
        }
    }

    pub fn start(&mut self, storage: SessionStorage) {
        self.active = true;
        self.tick_index = 0;
        self.ssrc_map.clear();
        self.storage = Some(storage);
        info!("Recording started");
    }

    pub fn stop(&mut self) -> Option<SessionStorage> {
        self.active = false;
        info!("Recording stopped at tick {}", self.tick_index);
        self.storage.take()
    }
}

impl Default for RecordingState {
    fn default() -> Self {
        Self::new()
    }
}

pub type SharedRecordingState = Arc<Mutex<RecordingState>>;

pub struct Receiver {
    state: SharedRecordingState,
}

impl Receiver {
    pub fn new(state: SharedRecordingState) -> Self {
        Self { state }
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
                if let Some(user_id) = user_id {
                    let mut state = self.state.lock().await;
                    state.ssrc_map.insert(*ssrc, user_id.0);
                    info!("Mapped SSRC {} to User ID {}", ssrc, user_id.0);
                }
            }
            EventContext::VoiceTick(VoiceTick {
                speaking,
                silent: _,
                ..
            }) => {
                let mut state = self.state.lock().await;

                if !state.active {
                    return None;
                }

                let current_tick = state.tick_index;
                state.tick_index += 1;

                for (ssrc, voice_data) in speaking {
                    let decoded = match &voice_data.decoded_voice {
                        Some(d) if !d.is_empty() => d,
                        _ => {
                            continue;
                        }
                    };

                    let stereo_sample_length = SAMPLES_PER_FRAME * 2;
                    let mono_sample_length = SAMPLES_PER_FRAME;
                    let mono_samples = match decoded.len() {
                        stereo_sample_length => stereo_to_mono(decoded),
                        mono_sample_length => decoded.clone(),
                        _ => {
                            warn!(
                                "Unexpected audio frame size: {} (expected {} or {})",
                                decoded.len(),
                                stereo_sample_length,
                                mono_sample_length
                            );
                            decoded.clone()
                        }
                    };

                    let frame = AudioFrame {
                        tick_index: current_tick,
                        samples: mono_samples,
                    };

                    info!("Writing audio frame for user {}: {:?}", ssrc, frame);
                    info!("SSRC map: {:?}", state.ssrc_map);
                    info!("Storage: {:?}", state.storage);
                    if let Some(&user_id) = state.ssrc_map.get(ssrc) {
                        if let Some(ref mut storage) = state.storage {
                            if let Err(e) = storage.write_frame(user_id, &frame) {
                                warn!("Failed to write audio frame for user {}: {}", user_id, e);
                            }
                        }
                    } else {
                        warn!("Received audio from unknown SSRC {}, skipping frame", ssrc);
                    }
                }
            }
            _ => {}
        }

        None
    }
}

pub fn create_recording_session() -> SharedRecordingState {
    Arc::new(Mutex::new(RecordingState::new()))
}
