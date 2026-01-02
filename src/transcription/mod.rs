//! Transcription module for speech-to-text.
//!
//! Provides integration with whisper.cpp for local STT processing.

pub mod transcript;
pub mod whisper;

pub use transcript::{Transcript, TranscriptSegment, TranscriptWord, ExportFormat};
pub use whisper::{WhisperModel, WhisperConfig, TranscriptionResult};

