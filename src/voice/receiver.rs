use super::audio::stereo_to_mono;
use super::storage::{AudioFrame, StorageHandle};
use songbird::{
    Event, EventContext, EventHandler, events::context_data::VoiceTick, model::payload::Speaking,
};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

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
                        Some(d) => d,
                        None => continue,
                    };

                    if decoded.is_empty() {
                        continue;
                    }

                    let samples = stereo_to_mono(decoded);

                    let mut is_every_sample_zero = true;
                    for sample in samples {
                        if sample != 0 {
                            is_every_sample_zero = false;
                            break;
                        }
                    }

                    if is_every_sample_zero {
                        continue;
                    }

                    if let Some(ref storage) = state.storage {
                        storage.buffer_frame(
                            *ssrc,
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
