use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use thiserror::Error;
use tracing::{info, warn};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use super::{AudioChunk, WHISPER_SAMPLE_RATE};

/// Available Whisper model sizes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhisperModel {
    Tiny,
    Base,
    Small,
    Medium,
    Large,
}

impl WhisperModel {
    /// Get the Hugging Face URL for this model
    pub fn hf_url(&self) -> &'static str {
        match self {
            WhisperModel::Tiny => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin",
            WhisperModel::Base => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
            WhisperModel::Small => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
            WhisperModel::Medium => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin",
            WhisperModel::Large => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin",
        }
    }

    /// Get the filename for this model
    pub fn filename(&self) -> &'static str {
        match self {
            WhisperModel::Tiny => "ggml-tiny.bin",
            WhisperModel::Base => "ggml-base.bin",
            WhisperModel::Small => "ggml-small.bin",
            WhisperModel::Medium => "ggml-medium.bin",
            WhisperModel::Large => "ggml-large-v3.bin",
        }
    }

    /// Get approximate model size in MB
    pub fn size_mb(&self) -> u64 {
        match self {
            WhisperModel::Tiny => 75,
            WhisperModel::Base => 142,
            WhisperModel::Small => 466,
            WhisperModel::Medium => 1500,
            WhisperModel::Large => 3100,
        }
    }
}

impl std::fmt::Display for WhisperModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WhisperModel::Tiny => write!(f, "tiny"),
            WhisperModel::Base => write!(f, "base"),
            WhisperModel::Small => write!(f, "small"),
            WhisperModel::Medium => write!(f, "medium"),
            WhisperModel::Large => write!(f, "large"),
        }
    }
}

impl std::str::FromStr for WhisperModel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "tiny" => Ok(WhisperModel::Tiny),
            "base" => Ok(WhisperModel::Base),
            "small" => Ok(WhisperModel::Small),
            "medium" => Ok(WhisperModel::Medium),
            "large" => Ok(WhisperModel::Large),
            _ => Err(format!("Unknown model: {}. Use tiny, base, small, medium, or large", s)),
        }
    }
}

#[derive(Error, Debug)]
pub enum WhisperError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Failed to download model: {0}")]
    Download(String),
    #[error("Failed to initialize Whisper: {0}")]
    Init(String),
    #[error("Transcription failed: {0}")]
    Transcription(String),
}

/// A single transcribed segment with timing
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TranscribedSegment {
    /// Start time in seconds (relative to chunk start)
    pub start_secs: f32,
    /// End time in seconds (relative to chunk start)
    pub end_secs: f32,
    /// The transcribed text
    pub text: String,
}

/// Result of transcribing an audio chunk
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChunkTranscription {
    /// Chunk index
    pub chunk_index: usize,
    /// Chunk start time (relative to user's audio start)
    pub chunk_start_secs: f32,
    /// Chunk end time
    pub chunk_end_secs: f32,
    /// Detected language
    pub language: Option<String>,
    /// Transcribed segments within the chunk
    pub segments: Vec<TranscribedSegment>,
    /// Full text (all segments joined)
    pub full_text: String,
}

/// Get the models directory path
pub fn models_dir() -> PathBuf {
    PathBuf::from("models").join("whisper")
}

/// Get the path to a specific model file
pub fn model_path(model: WhisperModel) -> PathBuf {
    models_dir().join(model.filename())
}

/// Check if a model is already downloaded
pub fn is_model_downloaded(model: WhisperModel) -> bool {
    let path = model_path(model);
    if !path.exists() {
        return false;
    }
    
    // Check if file size is reasonable (at least 50% of expected)
    if let Ok(metadata) = fs::metadata(&path) {
        let expected_bytes = model.size_mb() * 1024 * 1024;
        return metadata.len() >= expected_bytes / 2;
    }
    
    false
}

/// Download a Whisper model from Hugging Face
pub fn download_model(model: WhisperModel) -> Result<PathBuf, WhisperError> {
    let path = model_path(model);
    
    if is_model_downloaded(model) {
        info!("Model {} already downloaded at {:?}", model, path);
        return Ok(path);
    }

    // Create models directory
    fs::create_dir_all(models_dir())?;

    info!(
        "Downloading Whisper {} model (~{}MB)...",
        model,
        model.size_mb()
    );

    let url = model.hf_url();
    
    // Use blocking reqwest for simplicity
    let response = reqwest::blocking::Client::new()
        .get(url)
        .send()
        .map_err(|e| WhisperError::Download(format!("HTTP request failed: {}", e)))?;

    if !response.status().is_success() {
        return Err(WhisperError::Download(format!(
            "HTTP {} from {}",
            response.status(),
            url
        )));
    }

    let total_size = response.content_length().unwrap_or(0);
    
    // Create progress bar
    let pb = indicatif::ProgressBar::new(total_size);
    pb.set_style(
        indicatif::ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
            .unwrap()
            .progress_chars("#>-"),
    );

    // Download with progress
    let temp_path = path.with_extension("bin.tmp");
    let mut file = File::create(&temp_path)?;
    let mut downloaded: u64 = 0;
    
    let bytes = response.bytes()
        .map_err(|e| WhisperError::Download(format!("Failed to read response: {}", e)))?;
    
    file.write_all(&bytes)?;
    downloaded = bytes.len() as u64;
    pb.set_position(downloaded);
    
    pb.finish_with_message("Download complete");
    
    // Rename temp file to final path
    fs::rename(&temp_path, &path)?;
    
    info!("Model downloaded to {:?}", path);
    
    Ok(path)
}

/// Language configuration for transcription
#[derive(Debug, Clone)]
pub struct LanguageConfig {
    /// Primary language hint (None = auto-detect)
    /// Use "de" for German, "en" for English, or None for mixed
    pub language: Option<String>,
    /// Whether to translate to English (false = keep original language)
    pub translate: bool,
}

impl Default for LanguageConfig {
    fn default() -> Self {
        Self {
            // Auto-detect for mixed German/English
            language: None,
            translate: false,
        }
    }
}

impl LanguageConfig {
    /// Configure for mixed German/English speech (auto-detection)
    pub fn german_english_mixed() -> Self {
        Self {
            language: None, // Auto-detect each segment
            translate: false, // Keep original language
        }
    }
    
    /// Configure for primarily German with some English
    pub fn german_primary() -> Self {
        Self {
            language: Some("de".to_string()),
            translate: false,
        }
    }
    
    /// Configure for primarily English with some German
    pub fn english_primary() -> Self {
        Self {
            language: Some("en".to_string()),
            translate: false,
        }
    }
    
    /// Translate everything to English
    pub fn translate_to_english() -> Self {
        Self {
            language: None,
            translate: true,
        }
    }
}

/// Whisper transcriber
pub struct Transcriber {
    ctx: WhisperContext,
    model: WhisperModel,
    language_config: LanguageConfig,
    /// Number of threads to use (0 = auto)
    n_threads: i32,
}

impl Transcriber {
    /// Create a new transcriber with default language settings (auto-detect)
    pub fn new(model: WhisperModel) -> Result<Self, WhisperError> {
        Self::with_language(model, LanguageConfig::german_english_mixed())
    }
    
    /// Create a new transcriber with specific language configuration
    pub fn with_language(model: WhisperModel, language_config: LanguageConfig) -> Result<Self, WhisperError> {
        // Ensure model is downloaded
        let path = download_model(model)?;
        
        info!("Loading Whisper {} model...", model);
        
        let ctx = WhisperContext::new_with_params(
            path.to_str().unwrap(),
            WhisperContextParameters::default(),
        )
        .map_err(|e| WhisperError::Init(format!("Failed to load model: {}", e)))?;
        
        // Use available CPU threads (leave 1 for system)
        let n_threads = std::thread::available_parallelism()
            .map(|p| (p.get() as i32).max(1))
            .unwrap_or(4);
        
        info!("Whisper model loaded successfully (using {} threads)", n_threads);
        info!("Language config: {:?}", language_config);
        
        Ok(Self { ctx, model, language_config, n_threads })
    }

    /// Transcribe an audio chunk (optimized for speed)
    pub fn transcribe_chunk(&self, chunk: &AudioChunk) -> Result<ChunkTranscription, WhisperError> {
        let start_time = std::time::Instant::now();
        
        info!(
            "Transcribing chunk {} ({:.2}s audio)",
            chunk.index, chunk.duration_secs
        );

        // Use greedy sampling for speed (beam search is 2-3x slower)
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        
        // ===== SPEED OPTIMIZATIONS =====
        
        // Use multiple CPU threads
        params.set_n_threads(self.n_threads);
        
        // Single segment mode for shorter chunks (faster)
        if chunk.duration_secs < 30.0 {
            params.set_single_segment(true);
        }
        
        // Disable token-level timestamps (segment-level is enough, faster)
        params.set_token_timestamps(false);
        
        // ===== HALLUCINATION PREVENTION =====
        
        // Detect silence/no speech - skip segments with high no_speech probability
        params.set_no_speech_thold(0.6); // If >60% likely no speech, skip
        
        // Higher entropy threshold = more likely to stop on repetitive/uncertain output
        params.set_entropy_thold(2.4);
        
        // Log probability threshold - reject low confidence outputs
        params.set_logprob_thold(-1.0);
        
        // Temperature fallback for better quality (reduces hallucination)
        params.set_temperature(0.0); // Start with greedy (deterministic)
        params.set_temperature_inc(0.2); // Increase if decoding fails
        
        // Don't use previous context (prevents hallucination propagation)
        params.set_no_context(true);
        
        // Suppress non-speech tokens (music, noise descriptions)
        params.set_suppress_non_speech_tokens(true);
        
        // Limit max segment length to prevent long repetitive outputs
        params.set_max_len(80); // Max ~80 chars per segment
        
        // ===== LANGUAGE CONFIGURATION =====
        match &self.language_config.language {
            Some(lang) => params.set_language(Some(lang)),
            None => params.set_language(Some("auto")), // Auto-detect
        }
        
        // Translation setting
        params.set_translate(self.language_config.translate);
        
        // Print progress
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_print_special(false);
        
        // Create state and run
        let mut state = self.ctx.create_state()
            .map_err(|e| WhisperError::Transcription(format!("Failed to create state: {}", e)))?;
        
        // Run inference
        state
            .full(params, &chunk.samples)
            .map_err(|e| WhisperError::Transcription(format!("Inference failed: {}", e)))?;

        // Extract segments
        let num_segments = state.full_n_segments()
            .map_err(|e| WhisperError::Transcription(format!("Failed to get segments: {}", e)))?;
        
        let mut segments = Vec::new();
        let mut full_text = String::new();
        
        let mut last_text: Option<String> = None;
        let mut repeat_count = 0;
        const MAX_REPEATS: usize = 2; // Allow max 2 consecutive identical segments
        
        for i in 0..num_segments {
            let start_ts = state.full_get_segment_t0(i)
                .map_err(|e| WhisperError::Transcription(format!("Failed to get start time: {}", e)))?;
            let end_ts = state.full_get_segment_t1(i)
                .map_err(|e| WhisperError::Transcription(format!("Failed to get end time: {}", e)))?;
            let text = state.full_get_segment_text(i)
                .map_err(|e| WhisperError::Transcription(format!("Failed to get text: {}", e)))?;
            
            let text = text.trim().to_string();
            if text.is_empty() {
                continue;
            }
            
            // Timestamps are in centiseconds (1/100 second)
            let start_secs = start_ts as f32 / 100.0;
            let end_secs = end_ts as f32 / 100.0;
            
            // ===== HALLUCINATION DETECTION =====
            // Check for repeated text (hallucination symptom)
            let is_repeat = last_text.as_ref().map(|lt| lt == &text).unwrap_or(false);
            
            if is_repeat {
                repeat_count += 1;
                if repeat_count >= MAX_REPEATS {
                    // Skip this repeated segment - likely hallucination
                    continue;
                }
            } else {
                repeat_count = 0;
            }
            
            last_text = Some(text.clone());
            
            segments.push(TranscribedSegment {
                start_secs,
                end_secs,
                text: text.clone(),
            });
            
            if !full_text.is_empty() {
                full_text.push(' ');
            }
            full_text.push_str(&text);
        }
        
        // Try to get detected language
        let language = state.full_lang_id_from_state()
            .ok()
            .and_then(|id| whisper_rs::get_lang_str(id).map(|s| s.to_string()));

        let elapsed = start_time.elapsed();
        let realtime_factor = chunk.duration_secs / elapsed.as_secs_f32();
        
        // Log if we filtered hallucinations
        let filtered = num_segments as i32 - segments.len() as i32;
        if filtered > 0 {
            info!(
                "Transcribed chunk {} in {:.1}s ({:.1}x realtime): {} segments ({} hallucinations filtered)",
                chunk.index,
                elapsed.as_secs_f32(),
                realtime_factor,
                segments.len(),
                filtered
            );
        } else {
            info!(
                "Transcribed chunk {} in {:.1}s ({:.1}x realtime): {} segments",
                chunk.index,
                elapsed.as_secs_f32(),
                realtime_factor,
                segments.len()
            );
        }

        Ok(ChunkTranscription {
            chunk_index: chunk.index,
            chunk_start_secs: chunk.start_time_secs,
            chunk_end_secs: chunk.end_time_secs,
            language,
            segments,
            full_text,
        })
    }

    /// Transcribe multiple chunks
    /// Transcribe multiple chunks sequentially with progress tracking
    pub fn transcribe_chunks(&self, chunks: &[AudioChunk]) -> Result<Vec<ChunkTranscription>, WhisperError> {
        let total_audio_secs: f32 = chunks.iter().map(|c| c.duration_secs).sum();
        info!(
            "Transcribing {} chunks ({:.1}s total audio)...",
            chunks.len(),
            total_audio_secs
        );
        
        let start_time = std::time::Instant::now();
        let mut transcriptions = Vec::new();
        let mut processed_audio = 0.0f32;
        
        for (i, chunk) in chunks.iter().enumerate() {
            match self.transcribe_chunk(chunk) {
                Ok(t) => {
                    processed_audio += chunk.duration_secs;
                    let progress = (i + 1) as f32 / chunks.len() as f32 * 100.0;
                    let elapsed = start_time.elapsed().as_secs_f32();
                    let eta = if i > 0 {
                        elapsed / (i + 1) as f32 * (chunks.len() - i - 1) as f32
                    } else {
                        0.0
                    };
                    
                    info!(
                        "Progress: {:.0}% ({}/{}) - ETA: {:.0}s",
                        progress, i + 1, chunks.len(), eta
                    );
                    
                    transcriptions.push(t);
                }
                Err(e) => {
                    warn!("Failed to transcribe chunk {}: {}", chunk.index, e);
                    // Continue with other chunks
                }
            }
        }
        
        let total_elapsed = start_time.elapsed();
        let overall_realtime = total_audio_secs / total_elapsed.as_secs_f32();
        
        info!(
            "Completed {} chunks in {:.1}s ({:.1}x realtime overall)",
            transcriptions.len(),
            total_elapsed.as_secs_f32(),
            overall_realtime
        );
        
        Ok(transcriptions)
    }
    
    /// Get the model being used
    pub fn model(&self) -> WhisperModel {
        self.model
    }
    
    /// Get number of threads being used
    pub fn threads(&self) -> i32 {
        self.n_threads
    }
}

/// Full transcription result for a user
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UserTranscription {
    pub user_id: u64,
    pub display_name: String,
    pub model: String,
    pub total_duration_secs: f32,
    pub chunk_transcriptions: Vec<ChunkTranscription>,
    /// All segments with absolute timestamps (relative to user's audio start)
    pub all_segments: Vec<TranscribedSegment>,
    /// Full transcript text
    pub full_transcript: String,
}

impl UserTranscription {
    /// Create from chunk transcriptions, computing absolute timestamps
    pub fn from_chunks(
        user_id: u64,
        display_name: String,
        model: &str,
        total_duration_secs: f32,
        chunk_transcriptions: Vec<ChunkTranscription>,
    ) -> Self {
        let mut all_segments = Vec::new();
        let mut full_transcript = String::new();
        
        for ct in &chunk_transcriptions {
            for seg in &ct.segments {
                // Convert to absolute timestamps
                all_segments.push(TranscribedSegment {
                    start_secs: ct.chunk_start_secs + seg.start_secs,
                    end_secs: ct.chunk_start_secs + seg.end_secs,
                    text: seg.text.clone(),
                });
            }
            
            if !ct.full_text.is_empty() {
                if !full_transcript.is_empty() {
                    full_transcript.push(' ');
                }
                full_transcript.push_str(&ct.full_text);
            }
        }
        
        Self {
            user_id,
            display_name,
            model: model.to_string(),
            total_duration_secs,
            chunk_transcriptions,
            all_segments,
            full_transcript,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_parsing() {
        assert_eq!("tiny".parse::<WhisperModel>().unwrap(), WhisperModel::Tiny);
        assert_eq!("SMALL".parse::<WhisperModel>().unwrap(), WhisperModel::Small);
        assert!("invalid".parse::<WhisperModel>().is_err());
    }

    #[test]
    fn test_model_paths() {
        assert!(model_path(WhisperModel::Tiny).to_str().unwrap().contains("ggml-tiny.bin"));
    }
}

