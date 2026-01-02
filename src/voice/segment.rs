//! Speech segmentation engine.
//!
//! Builds speech segments from consecutive ticks, allowing small gaps
//! to handle network jitter.

use super::storage::AudioFrame;
use std::time::Duration;

/// Configuration for segmentation
#[derive(Debug, Clone)]
pub struct SegmentConfig {
    /// Maximum gap in ticks to bridge (default: 2 = 40ms)
    pub max_gap_ticks: u64,
    /// Minimum segment length in ticks (default: 25 = 500ms)
    pub min_segment_ticks: u64,
    /// Maximum segment length in ticks (default: 2250 = 45s)
    pub max_segment_ticks: u64,
    /// Target segment length in ticks (default: 1500 = 30s)
    pub target_segment_ticks: u64,
    /// Overlap in ticks for STT (default: 25 = 500ms)
    pub overlap_ticks: u64,
    /// Duration per tick in milliseconds
    pub tick_duration_ms: f32,
}

impl Default for SegmentConfig {
    fn default() -> Self {
        Self {
            max_gap_ticks: 2,           // 40ms - handle jitter
            min_segment_ticks: 25,       // 500ms minimum
            max_segment_ticks: 2250,     // 45s maximum
            target_segment_ticks: 1500,  // 30s target
            overlap_ticks: 25,           // 500ms overlap
            tick_duration_ms: 20.0,
        }
    }
}

impl SegmentConfig {
    /// Convert ticks to seconds
    pub fn ticks_to_secs(&self, ticks: u64) -> f64 {
        ticks as f64 * self.tick_duration_ms as f64 / 1000.0
    }

    /// Convert ticks to Duration
    pub fn ticks_to_duration(&self, ticks: u64) -> Duration {
        Duration::from_secs_f64(self.ticks_to_secs(ticks))
    }
}

/// A speech segment with timing information
#[derive(Debug, Clone)]
pub struct SpeechSegment {
    /// Segment ID (sequential)
    pub id: usize,
    /// Start tick index
    pub start_tick: u64,
    /// End tick index (inclusive)
    pub end_tick: u64,
    /// Frames in this segment
    pub frames: Vec<AudioFrame>,
    /// User ID this segment belongs to
    pub user_id: u64,
}

impl SpeechSegment {
    /// Start time in seconds
    pub fn start_secs(&self, tick_duration_ms: f32) -> f64 {
        self.start_tick as f64 * tick_duration_ms as f64 / 1000.0
    }

    /// End time in seconds
    pub fn end_secs(&self, tick_duration_ms: f32) -> f64 {
        (self.end_tick + 1) as f64 * tick_duration_ms as f64 / 1000.0
    }

    /// Duration in seconds
    pub fn duration_secs(&self, tick_duration_ms: f32) -> f64 {
        self.end_secs(tick_duration_ms) - self.start_secs(tick_duration_ms)
    }

    /// Number of ticks
    pub fn tick_count(&self) -> u64 {
        self.end_tick - self.start_tick + 1
    }
}

/// Segments audio frames into speech segments
pub struct Segmenter {
    config: SegmentConfig,
}

impl Segmenter {
    pub fn new(config: SegmentConfig) -> Self {
        Self { config }
    }

    pub fn with_defaults() -> Self {
        Self::new(SegmentConfig::default())
    }

    /// Segment frames for a single user
    pub fn segment_frames(&self, user_id: u64, frames: &[AudioFrame]) -> Vec<SpeechSegment> {
        if frames.is_empty() {
            return Vec::new();
        }

        let mut segments = Vec::new();
        let mut current_segment_frames: Vec<AudioFrame> = Vec::new();
        let mut current_start_tick: Option<u64> = None;
        let mut last_tick: Option<u64> = None;
        let mut segment_id = 0;

        for frame in frames {
            let should_start_new = match last_tick {
                None => true,
                Some(lt) => {
                    let gap = frame.tick_index.saturating_sub(lt);
                    gap > self.config.max_gap_ticks + 1
                }
            };

            if should_start_new {
                // Finalize previous segment if it meets minimum length
                if let Some(start) = current_start_tick {
                    if let Some(lt) = last_tick {
                        let tick_count = lt - start + 1;
                        if tick_count >= self.config.min_segment_ticks {
                            segments.push(SpeechSegment {
                                id: segment_id,
                                start_tick: start,
                                end_tick: lt,
                                frames: std::mem::take(&mut current_segment_frames),
                                user_id,
                            });
                            segment_id += 1;
                        }
                    }
                }

                // Start new segment
                current_start_tick = Some(frame.tick_index);
                current_segment_frames.clear();
            }

            current_segment_frames.push(frame.clone());
            last_tick = Some(frame.tick_index);

            // Check if segment is too long and needs splitting
            if let Some(start) = current_start_tick {
                let current_length = frame.tick_index - start + 1;
                if current_length >= self.config.max_segment_ticks {
                    // Split at target length
                    segments.push(SpeechSegment {
                        id: segment_id,
                        start_tick: start,
                        end_tick: frame.tick_index,
                        frames: std::mem::take(&mut current_segment_frames),
                        user_id,
                    });
                    segment_id += 1;
                    current_start_tick = None;
                    last_tick = None;
                }
            }
        }

        // Finalize last segment
        if let Some(start) = current_start_tick {
            if let Some(lt) = last_tick {
                let tick_count = lt - start + 1;
                if tick_count >= self.config.min_segment_ticks || !segments.is_empty() {
                    // Include even short final segments if we have other segments
                    segments.push(SpeechSegment {
                        id: segment_id,
                        start_tick: start,
                        end_tick: lt,
                        frames: current_segment_frames,
                        user_id,
                    });
                }
            }
        }

        segments
    }

    /// Create overlapping segments for STT (improves transcription at boundaries)
    pub fn create_overlapping_segments(
        &self,
        segments: &[SpeechSegment],
    ) -> Vec<SpeechSegment> {
        if segments.len() < 2 {
            return segments.to_vec();
        }

        let mut result = Vec::with_capacity(segments.len());

        for (i, segment) in segments.iter().enumerate() {
            let mut new_segment = segment.clone();

            // Add overlap from next segment
            if i + 1 < segments.len() {
                let next = &segments[i + 1];
                let overlap_end = next.start_tick + self.config.overlap_ticks;

                // Find frames from next segment that fall within overlap
                let overlap_frames: Vec<_> = next
                    .frames
                    .iter()
                    .filter(|f| f.tick_index < overlap_end)
                    .cloned()
                    .collect();

                if !overlap_frames.is_empty() {
                    new_segment.frames.extend(overlap_frames);
                    new_segment.end_tick = new_segment
                        .frames
                        .last()
                        .map(|f| f.tick_index)
                        .unwrap_or(new_segment.end_tick);
                }
            }

            result.push(new_segment);
        }

        result
    }

    pub fn config(&self) -> &SegmentConfig {
        &self.config
    }
}

/// Statistics about segmentation results
#[derive(Debug, Default)]
pub struct SegmentStats {
    pub total_segments: usize,
    pub total_duration_secs: f64,
    pub avg_segment_duration_secs: f64,
    pub min_segment_duration_secs: f64,
    pub max_segment_duration_secs: f64,
    pub total_frames: usize,
}

impl SegmentStats {
    pub fn compute(segments: &[SpeechSegment], tick_duration_ms: f32) -> Self {
        if segments.is_empty() {
            return Self::default();
        }

        let durations: Vec<f64> = segments
            .iter()
            .map(|s| s.duration_secs(tick_duration_ms))
            .collect();

        let total_duration_secs: f64 = durations.iter().sum();
        let total_frames: usize = segments.iter().map(|s| s.frames.len()).sum();

        Self {
            total_segments: segments.len(),
            total_duration_secs,
            avg_segment_duration_secs: total_duration_secs / segments.len() as f64,
            min_segment_duration_secs: durations
                .iter()
                .cloned()
                .min_by(|a, b| a.partial_cmp(b).unwrap())
                .unwrap_or(0.0),
            max_segment_duration_secs: durations
                .iter()
                .cloned()
                .max_by(|a, b| a.partial_cmp(b).unwrap())
                .unwrap_or(0.0),
            total_frames,
        }
    }
}
