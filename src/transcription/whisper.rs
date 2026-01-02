//! Whisper.cpp integration for local speech-to-text.
//!
//! Uses the whisper-rs crate which provides Rust bindings to whisper.cpp.

use super::transcript::{TranscriptSegment, TranscriptWord};
use std::path::Path;
use tracing::{info, warn, error};

/// Whisper model configuration
#[derive(Debug, Clone)]
pub struct WhisperConfig {
    /// Path to the model file (.bin)
    pub model_path: String,
    /// Language code (e.g., "en", "auto" for detection)
    pub language: Option<String>,
    /// Enable translation to English
    pub translate: bool,
    /// Number of threads for processing
    pub n_threads: i32,
    /// Enable word-level timestamps
    pub word_timestamps: bool,
    /// Maximum segment length in characters (for splitting)
    pub max_segment_len: usize,
}

impl Default for WhisperConfig {
    fn default() -> Self {
        Self {
            model_path: "models/ggml-base.en.bin".to_string(),
            language: Some("en".to_string()),
            translate: false,
            n_threads: 4,
            word_timestamps: true,
            max_segment_len: 0, // No splitting
        }
    }
}

impl WhisperConfig {
    pub fn with_model(model_path: impl Into<String>) -> Self {
        Self {
            model_path: model_path.into(),
            ..Default::default()
        }
    }

    pub fn auto_detect_language(mut self) -> Self {
        self.language = None;
        self
    }

    pub fn with_threads(mut self, n: i32) -> Self {
        self.n_threads = n;
        self
    }
}

/// Result from transcription
#[derive(Debug, Clone)]
pub struct TranscriptionResult {
    /// Transcribed segments
    pub segments: Vec<TranscriptSegment>,
    /// Detected language (if auto-detect was used)
    pub detected_language: Option<String>,
    /// Processing time in seconds
    pub processing_time_secs: f64,
}

/// Whisper model wrapper
/// 
/// Note: This is a placeholder implementation. The actual whisper-rs
/// integration requires the whisper-rs crate and a compiled whisper.cpp
/// library. For production, you would use:
/// 
/// ```ignore
/// use whisper_rs::{WhisperContext, WhisperContextParameters, FullParams, SamplingStrategy};
/// ```
pub struct WhisperModel {
    config: WhisperConfig,
    // In production: ctx: WhisperContext,
}

impl WhisperModel {
    /// Load a Whisper model from file
    pub fn load(config: WhisperConfig) -> Result<Self, WhisperError> {
        let model_path = Path::new(&config.model_path);
        
        if !model_path.exists() {
            return Err(WhisperError::ModelNotFound(config.model_path.clone()));
        }

        info!("Loading Whisper model from: {}", config.model_path);

        // In production, this would be:
        // let ctx = WhisperContext::new_with_params(
        //     &config.model_path,
        //     WhisperContextParameters::default(),
        // )?;

        Ok(Self {
            config,
            // ctx,
        })
    }

    /// Transcribe audio samples (16kHz, mono, f32)
    pub fn transcribe(&self, samples: &[f32]) -> Result<TranscriptionResult, WhisperError> {
        let start_time = std::time::Instant::now();

        info!(
            "Transcribing {} samples ({:.2}s of audio)",
            samples.len(),
            samples.len() as f64 / 16000.0
        );

        // In production, this would use whisper-rs:
        // 
        // let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        // params.set_n_threads(self.config.n_threads);
        // params.set_language(self.config.language.as_deref());
        // params.set_translate(self.config.translate);
        // params.set_token_timestamps(self.config.word_timestamps);
        // 
        // let mut state = self.ctx.create_state()?;
        // state.full(params, samples)?;
        // 
        // let num_segments = state.full_n_segments()?;
        // let mut segments = Vec::new();
        // 
        // for i in 0..num_segments {
        //     let start = state.full_get_segment_t0(i)? as f64 / 100.0;
        //     let end = state.full_get_segment_t1(i)? as f64 / 100.0;
        //     let text = state.full_get_segment_text(i)?;
        //     
        //     segments.push(TranscriptSegment {
        //         id: i as usize,
        //         start,
        //         end,
        //         text,
        //         words: None,
        //         speaker_id: None,
        //         speaker_name: None,
        //     });
        // }

        // Placeholder implementation - returns empty result
        // Remove this when integrating actual whisper-rs
        let segments = Vec::new();
        
        warn!("Whisper transcription is a placeholder - integrate whisper-rs for actual STT");

        let processing_time = start_time.elapsed().as_secs_f64();

        Ok(TranscriptionResult {
            segments,
            detected_language: self.config.language.clone(),
            processing_time_secs: processing_time,
        })
    }

    /// Transcribe from i16 PCM samples (will convert to f32)
    pub fn transcribe_pcm(&self, samples: &[i16]) -> Result<TranscriptionResult, WhisperError> {
        // Convert i16 to f32 normalized
        let float_samples: Vec<f32> = samples
            .iter()
            .map(|&s| s as f32 / i16::MAX as f32)
            .collect();

        self.transcribe(&float_samples)
    }

    pub fn config(&self) -> &WhisperConfig {
        &self.config
    }
}

/// Errors that can occur during Whisper operations
#[derive(Debug)]
pub enum WhisperError {
    /// Model file not found
    ModelNotFound(String),
    /// Failed to load model
    LoadError(String),
    /// Transcription failed
    TranscriptionError(String),
    /// Invalid audio format
    InvalidAudio(String),
}

impl std::fmt::Display for WhisperError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WhisperError::ModelNotFound(path) => {
                write!(f, "Whisper model not found: {}", path)
            }
            WhisperError::LoadError(msg) => {
                write!(f, "Failed to load Whisper model: {}", msg)
            }
            WhisperError::TranscriptionError(msg) => {
                write!(f, "Transcription failed: {}", msg)
            }
            WhisperError::InvalidAudio(msg) => {
                write!(f, "Invalid audio format: {}", msg)
            }
        }
    }
}

impl std::error::Error for WhisperError {}

/// Download a Whisper model from Hugging Face
pub async fn download_model(model_name: &str, output_dir: &Path) -> Result<std::path::PathBuf, WhisperError> {
    // Model URLs from Hugging Face
    let model_url = match model_name {
        "tiny" | "tiny.en" => {
            format!(
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{}.bin",
                model_name
            )
        }
        "base" | "base.en" => {
            format!(
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{}.bin",
                model_name
            )
        }
        "small" | "small.en" => {
            format!(
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{}.bin",
                model_name
            )
        }
        "medium" | "medium.en" => {
            format!(
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{}.bin",
                model_name
            )
        }
        "large" | "large-v2" | "large-v3" => {
            format!(
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{}.bin",
                model_name
            )
        }
        _ => {
            return Err(WhisperError::ModelNotFound(format!(
                "Unknown model: {}",
                model_name
            )));
        }
    };

    let output_path = output_dir.join(format!("ggml-{}.bin", model_name));

    if output_path.exists() {
        info!("Model already exists: {:?}", output_path);
        return Ok(output_path);
    }

    info!("Downloading model {} to {:?}", model_name, output_path);

    // Create output directory
    std::fs::create_dir_all(output_dir)
        .map_err(|e| WhisperError::LoadError(format!("Failed to create directory: {}", e)))?;

    // Download using reqwest (would need to add reqwest to dependencies)
    // For now, return an error asking user to download manually
    Err(WhisperError::LoadError(format!(
        "Please download the model manually:\n\
        curl -L {} -o {:?}",
        model_url, output_path
    )))
}

/// Check if a model file exists and is valid
pub fn validate_model(path: &Path) -> Result<(), WhisperError> {
    if !path.exists() {
        return Err(WhisperError::ModelNotFound(
            path.to_string_lossy().to_string(),
        ));
    }

    let metadata = std::fs::metadata(path)
        .map_err(|e| WhisperError::LoadError(format!("Failed to read model file: {}", e)))?;

    // Basic size check (models should be at least 30MB)
    if metadata.len() < 30_000_000 {
        return Err(WhisperError::LoadError(
            "Model file appears to be corrupted or incomplete".to_string(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_whisper_config_default() {
        let config = WhisperConfig::default();
        assert_eq!(config.language, Some("en".to_string()));
        assert!(!config.translate);
        assert!(config.word_timestamps);
    }

    #[test]
    fn test_whisper_config_builder() {
        let config = WhisperConfig::with_model("path/to/model.bin")
            .auto_detect_language()
            .with_threads(8);

        assert_eq!(config.model_path, "path/to/model.bin");
        assert_eq!(config.language, None);
        assert_eq!(config.n_threads, 8);
    }
}

