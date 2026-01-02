use super::audio::{prepare_for_transcription, rebuild_pcm_from_frames, save_wav, AudioFormat};
use super::segment::{Segmenter, SegmentConfig, SpeechSegment, SegmentStats};
use super::storage::{SparseAudioReader, AudioFrame};
use crate::transcription::{Transcript, TranscriptSegment, ExportFormat};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use tracing::{info, error};

#[derive(Debug, Clone)]
pub struct ExportConfig {
    pub output_dir: PathBuf,
    pub per_user_wav: bool,
    pub mixed_wav: bool,
    pub prepare_for_stt: bool,
    pub transcript_formats: Vec<ExportFormat>,
}

impl Default for ExportConfig {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from("exports"),
            per_user_wav: true,
            mixed_wav: true,
            prepare_for_stt: true,
            transcript_formats: vec![ExportFormat::JsonPretty, ExportFormat::Vtt],
        }
    }
}

#[derive(Debug)]
pub struct ExportResult {
    pub mixed_wav_path: Option<PathBuf>,
    pub user_wav_paths: HashMap<u64, PathBuf>,
    pub stt_segment_paths: Vec<PathBuf>,
    pub transcript_paths: HashMap<String, PathBuf>,
    pub total_duration_secs: f64,
    pub user_count: usize,
}

pub struct SessionExporter {
    config: ExportConfig,
    segmenter: Segmenter,
}

impl SessionExporter {
    pub fn new(config: ExportConfig) -> Self {
        Self {
            config,
            segmenter: Segmenter::with_defaults(),
        }
    }

    pub fn with_segment_config(mut self, segment_config: SegmentConfig) -> Self {
        self.segmenter = Segmenter::new(segment_config);
        self
    }

    pub fn export_session(
        &self,
        session_dir: &Path,
        session_id: &str,
    ) -> io::Result<ExportResult> {
        let output_dir = self.config.output_dir.join(session_id);
        std::fs::create_dir_all(&output_dir)?;

        info!("Exporting session {} to {:?}", session_id, output_dir);

        let user_files = self.find_user_files(session_dir)?;
        
        if user_files.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "No audio files found in session directory",
            ));
        }

        let mut result = ExportResult {
            mixed_wav_path: None,
            user_wav_paths: HashMap::new(),
            stt_segment_paths: Vec::new(),
            transcript_paths: HashMap::new(),
            total_duration_secs: 0.0,
            user_count: user_files.len(),
        };

        let mut all_user_pcm: HashMap<u64, Vec<i16>> = HashMap::new();
        let mut all_segments: Vec<SpeechSegment> = Vec::new();
        let mut max_duration_ticks: u64 = 0;

        for (user_id, file_path) in &user_files {
            info!("Processing user {} from {:?}", user_id, file_path);

            let mut reader = SparseAudioReader::open(file_path)?;
            let sample_rate = reader.header().sample_rate;
            let channels = reader.header().channels;
            let frames = reader.read_all_frames()?;

            if frames.is_empty() {
                continue;
            }

            if let Some(last_frame) = frames.last() {
                max_duration_ticks = max_duration_ticks.max(last_frame.tick_index + 1);
            }

            let segments = self.segmenter.segment_frames(*user_id, &frames);
            let stats = SegmentStats::compute(&segments, 20.0);
            
            info!(
                "User {}: {} segments, {:.1}s total, {:.1}s avg",
                user_id,
                stats.total_segments,
                stats.total_duration_secs,
                stats.avg_segment_duration_secs
            );

            all_segments.extend(segments);

            let samples_per_tick = (sample_rate as f32 * 0.020) as usize;
            let pcm = rebuild_pcm_from_frames(&frames, samples_per_tick);

            if self.config.per_user_wav {
                let wav_path = output_dir.join(format!("user_{}.wav", user_id));
                save_wav(&pcm, wav_path.to_str().unwrap(), sample_rate, channels)
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("WAV write error: {}", e)))?;
                result.user_wav_paths.insert(*user_id, wav_path);
            }

            all_user_pcm.insert(*user_id, pcm);
        }

        result.total_duration_secs = max_duration_ticks as f64 * 0.020;

        if self.config.mixed_wav && !all_user_pcm.is_empty() {
            let mixed = self.mix_user_audio(&all_user_pcm);
            let mixed_path = output_dir.join("mixed.wav");
            save_wav(&mixed, mixed_path.to_str().unwrap(), 48000, 1)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("WAV write error: {}", e)))?;
            result.mixed_wav_path = Some(mixed_path);
        }

        if self.config.prepare_for_stt && !all_segments.is_empty() {
            let stt_dir = output_dir.join("stt_segments");
            std::fs::create_dir_all(&stt_dir)?;

            for segment in &all_segments {
                let pcm = rebuild_pcm_from_frames(&segment.frames, 960);
                let stt_audio = prepare_for_transcription(&pcm, AudioFormat::CAPTURE_MONO);

                let segment_path = stt_dir.join(format!(
                    "segment_{}_{}.wav",
                    segment.user_id, segment.id
                ));

                save_wav(&stt_audio, segment_path.to_str().unwrap(), 16000, 1)
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("WAV write error: {}", e)))?;
                result.stt_segment_paths.push(segment_path);
            }
        }

        info!(
            "Export complete: {} users, {:.1}s duration, {} segments",
            result.user_count,
            result.total_duration_secs,
            all_segments.len()
        );

        Ok(result)
    }

    /// Find all user audio files in a session directory
    fn find_user_files(&self, session_dir: &Path) -> io::Result<Vec<(u64, PathBuf)>> {
        let mut files = Vec::new();

        for entry in std::fs::read_dir(session_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().map_or(false, |ext| ext == "wrty") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    if let Some(user_id_str) = stem.strip_prefix("user_") {
                        if let Ok(user_id) = user_id_str.parse::<u64>() {
                            files.push((user_id, path));
                        }
                    }
                }
            }
        }

        Ok(files)
    }

    /// Mix multiple user audio tracks
    fn mix_user_audio(&self, user_pcm: &HashMap<u64, Vec<i16>>) -> Vec<i16> {
        if user_pcm.is_empty() {
            return Vec::new();
        }

        let max_len = user_pcm.values().map(|p| p.len()).max().unwrap_or(0);
        let mut mixed = vec![0i32; max_len];

        for pcm in user_pcm.values() {
            for (i, &sample) in pcm.iter().enumerate() {
                mixed[i] += sample as i32;
            }
        }

        let num_users = user_pcm.len() as i32;
        mixed
            .into_iter()
            .map(|s| (s / num_users).clamp(i16::MIN as i32, i16::MAX as i32) as i16)
            .collect()
    }

    /// Export transcript to multiple formats
    pub fn export_transcript(
        &self,
        transcript: &Transcript,
        output_dir: &Path,
        session_id: &str,
    ) -> io::Result<HashMap<String, PathBuf>> {
        let mut paths = HashMap::new();

        for format in &self.config.transcript_formats {
            let filename = format!("{}.{}", session_id, format.extension());
            let path = output_dir.join(filename);

            transcript.save_to_file(&path, *format)?;
            paths.insert(format.extension().to_string(), path);
        }

        Ok(paths)
    }
}

/// Quick export function for simple cases
pub fn quick_export(
    session_dir: &Path,
    output_dir: &Path,
    session_id: &str,
) -> io::Result<ExportResult> {
    let config = ExportConfig {
        output_dir: output_dir.to_path_buf(),
        ..Default::default()
    };

    let exporter = SessionExporter::new(config);
    exporter.export_session(session_dir, session_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_export_config_default() {
        let config = ExportConfig::default();
        assert!(config.per_user_wav);
        assert!(config.mixed_wav);
        assert!(config.prepare_for_stt);
    }
}

