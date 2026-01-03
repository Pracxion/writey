use super::audio::stereo_to_mono;
use super::storage::{AudioFrame, StorageHandle};
use songbird::{
    Event, EventContext, EventHandler, events::context_data::VoiceTick, model::payload::Speaking,
};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;
use tracing::info;

/// Sample rate for audio (48kHz is Discord's native rate)
pub const SAMPLE_RATE: u32 = 48000;
/// Number of channels for capture (mono for speech)
pub const CHANNELS: u16 = 1;
/// Frame duration in milliseconds
pub const FRAME_DURATION_MS: f32 = 20.0;
/// Samples per 20ms frame at 48kHz mono
pub const SAMPLES_PER_FRAME: usize = (SAMPLE_RATE as f32 * FRAME_DURATION_MS / 1000.0) as usize;

pub struct RecordingState {
    pub active: bool,
    pub tick_index: u64,
    pub ssrc_map: HashMap<u32, u64>,
    pub storage: Option<StorageHandle>,
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

    pub fn start(&mut self, storage: StorageHandle) {
        self.active = true;
        self.tick_index = 0;
        self.ssrc_map.clear();
        self.storage = Some(storage);
    }

    pub fn stop(&mut self) -> Option<StorageHandle> {
        self.active = false;
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

                    if let Some(ref storage) = state.storage {
                        storage.update_ssrc_map(state.ssrc_map.clone());
                    }
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

                    if let Some(ref storage) = state.storage {
                        storage.buffer_frame(
                            *ssrc as u64,
                            AudioFrame {
                                tick_index: current_tick,
                                samples: stereo_to_mono(decoded),
                            },
                        );
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
