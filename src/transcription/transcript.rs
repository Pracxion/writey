//! Transcript types and export formatters.
//!
//! Supports JSON, SRT, and VTT output formats.

use serde::{Deserialize, Serialize};
use std::fmt::Write as FmtWrite;
use std::io::{self, Write};
use std::path::Path;

/// A word with timing information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptWord {
    /// The word text
    pub text: String,
    /// Start time in seconds
    pub start: f64,
    /// End time in seconds
    pub end: f64,
    /// Confidence score (0.0 - 1.0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
}

/// A segment of transcribed speech
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptSegment {
    /// Segment ID
    pub id: usize,
    /// Start time in seconds
    pub start: f64,
    /// End time in seconds
    pub end: f64,
    /// Transcribed text
    pub text: String,
    /// Individual words (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub words: Option<Vec<TranscriptWord>>,
    /// Speaker/user ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_id: Option<u64>,
    /// Speaker name (if known)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_name: Option<String>,
}

impl TranscriptSegment {
    /// Duration of the segment in seconds
    pub fn duration(&self) -> f64 {
        self.end - self.start
    }
}

/// Complete transcript for a recording session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcript {
    /// Session metadata
    pub metadata: TranscriptMetadata,
    /// All segments
    pub segments: Vec<TranscriptSegment>,
}

/// Metadata about the transcript
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptMetadata {
    /// Session ID or filename
    pub session_id: String,
    /// Recording start time (ISO 8601)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recorded_at: Option<String>,
    /// Total duration in seconds
    pub duration_secs: f64,
    /// Guild ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<String>,
    /// Model used for transcription
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Language detected/used
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

impl Transcript {
    pub fn new(session_id: String, duration_secs: f64) -> Self {
        Self {
            metadata: TranscriptMetadata {
                session_id,
                recorded_at: Some(chrono::Utc::now().to_rfc3339()),
                duration_secs,
                guild_id: None,
                model: None,
                language: None,
            },
            segments: Vec::new(),
        }
    }

    /// Add a segment to the transcript
    pub fn add_segment(&mut self, segment: TranscriptSegment) {
        self.segments.push(segment);
    }

    /// Get full text (all segments concatenated)
    pub fn full_text(&self) -> String {
        self.segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Export to the specified format
    pub fn export(&self, format: ExportFormat) -> String {
        match format {
            ExportFormat::Json => self.to_json(),
            ExportFormat::JsonPretty => self.to_json_pretty(),
            ExportFormat::Srt => self.to_srt(),
            ExportFormat::Vtt => self.to_vtt(),
            ExportFormat::Text => self.to_text(),
        }
    }

    /// Export to JSON
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Export to pretty-printed JSON
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Export to SRT format
    pub fn to_srt(&self) -> String {
        let mut output = String::new();

        for (i, segment) in self.segments.iter().enumerate() {
            let _ = writeln!(output, "{}", i + 1);
            let _ = writeln!(
                output,
                "{} --> {}",
                format_srt_time(segment.start),
                format_srt_time(segment.end)
            );
            
            // Add speaker prefix if available
            if let Some(ref name) = segment.speaker_name {
                let _ = writeln!(output, "[{}] {}", name, segment.text);
            } else if let Some(id) = segment.speaker_id {
                let _ = writeln!(output, "[User {}] {}", id, segment.text);
            } else {
                let _ = writeln!(output, "{}", segment.text);
            }
            
            let _ = writeln!(output);
        }

        output
    }

    /// Export to WebVTT format
    pub fn to_vtt(&self) -> String {
        let mut output = String::from("WEBVTT\n\n");

        for (i, segment) in self.segments.iter().enumerate() {
            let _ = writeln!(output, "{}", i + 1);
            let _ = writeln!(
                output,
                "{} --> {}",
                format_vtt_time(segment.start),
                format_vtt_time(segment.end)
            );

            // Add speaker prefix if available
            if let Some(ref name) = segment.speaker_name {
                let _ = writeln!(output, "<v {}>{}", name, segment.text);
            } else if let Some(id) = segment.speaker_id {
                let _ = writeln!(output, "<v User {}>{}", id, segment.text);
            } else {
                let _ = writeln!(output, "{}", segment.text);
            }

            let _ = writeln!(output);
        }

        output
    }

    /// Export to plain text
    pub fn to_text(&self) -> String {
        let mut output = String::new();

        for segment in &self.segments {
            let timestamp = format_timestamp(segment.start);
            
            if let Some(ref name) = segment.speaker_name {
                let _ = writeln!(output, "[{}] {}: {}", timestamp, name, segment.text);
            } else if let Some(id) = segment.speaker_id {
                let _ = writeln!(output, "[{}] User {}: {}", timestamp, id, segment.text);
            } else {
                let _ = writeln!(output, "[{}] {}", timestamp, segment.text);
            }
        }

        output
    }

    /// Save to file
    pub fn save_to_file(&self, path: &Path, format: ExportFormat) -> io::Result<()> {
        let content = self.export(format);
        std::fs::write(path, content)
    }
}

/// Export format options
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    /// Compact JSON
    Json,
    /// Pretty-printed JSON
    JsonPretty,
    /// SubRip subtitle format
    Srt,
    /// WebVTT subtitle format
    Vtt,
    /// Plain text with timestamps
    Text,
}

impl ExportFormat {
    /// Get file extension for this format
    pub fn extension(&self) -> &'static str {
        match self {
            ExportFormat::Json | ExportFormat::JsonPretty => "json",
            ExportFormat::Srt => "srt",
            ExportFormat::Vtt => "vtt",
            ExportFormat::Text => "txt",
        }
    }

    /// Parse format from string
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "json" => Some(ExportFormat::Json),
            "json-pretty" | "json_pretty" => Some(ExportFormat::JsonPretty),
            "srt" => Some(ExportFormat::Srt),
            "vtt" | "webvtt" => Some(ExportFormat::Vtt),
            "txt" | "text" => Some(ExportFormat::Text),
            _ => None,
        }
    }
}

/// Format time for SRT (HH:MM:SS,mmm)
fn format_srt_time(seconds: f64) -> String {
    let total_ms = (seconds * 1000.0) as u64;
    let ms = total_ms % 1000;
    let total_secs = total_ms / 1000;
    let secs = total_secs % 60;
    let total_mins = total_secs / 60;
    let mins = total_mins % 60;
    let hours = total_mins / 60;

    format!("{:02}:{:02}:{:02},{:03}", hours, mins, secs, ms)
}

/// Format time for VTT (HH:MM:SS.mmm)
fn format_vtt_time(seconds: f64) -> String {
    let total_ms = (seconds * 1000.0) as u64;
    let ms = total_ms % 1000;
    let total_secs = total_ms / 1000;
    let secs = total_secs % 60;
    let total_mins = total_secs / 60;
    let mins = total_mins % 60;
    let hours = total_mins / 60;

    format!("{:02}:{:02}:{:02}.{:03}", hours, mins, secs, ms)
}

/// Format timestamp for text output (MM:SS)
fn format_timestamp(seconds: f64) -> String {
    let total_secs = seconds as u64;
    let secs = total_secs % 60;
    let mins = total_secs / 60;

    if mins >= 60 {
        let hours = mins / 60;
        let mins = mins % 60;
        format!("{:02}:{:02}:{:02}", hours, mins, secs)
    } else {
        format!("{:02}:{:02}", mins, secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_srt_time_format() {
        assert_eq!(format_srt_time(0.0), "00:00:00,000");
        assert_eq!(format_srt_time(1.5), "00:00:01,500");
        assert_eq!(format_srt_time(65.123), "00:01:05,123");
        assert_eq!(format_srt_time(3661.5), "01:01:01,500");
    }

    #[test]
    fn test_vtt_time_format() {
        assert_eq!(format_vtt_time(0.0), "00:00:00.000");
        assert_eq!(format_vtt_time(1.5), "00:00:01.500");
    }

    #[test]
    fn test_transcript_export() {
        let mut transcript = Transcript::new("test".to_string(), 10.0);
        transcript.add_segment(TranscriptSegment {
            id: 0,
            start: 0.0,
            end: 2.5,
            text: "Hello world".to_string(),
            words: None,
            speaker_id: Some(12345),
            speaker_name: Some("Alice".to_string()),
        });

        let srt = transcript.to_srt();
        assert!(srt.contains("Hello world"));
        assert!(srt.contains("00:00:00,000"));
        assert!(srt.contains("Alice"));

        let vtt = transcript.to_vtt();
        assert!(vtt.starts_with("WEBVTT"));
        assert!(vtt.contains("00:00:00.000"));
    }
}

