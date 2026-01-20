use crate::db;
use crate::transcribe::{
    prepare_session_for_transcription, AudioChunk, LanguageConfig, PreparedAudio, Transcriber,
    UserTranscription, WhisperModel, MIN_SILENCE_DURATION_SECS,
};
use crate::Context;
use crate::Error;
use std::fs;
use std::path::PathBuf;
use tracing::info;

/// Parse language mode string into LanguageConfig
fn parse_language_mode(mode: Option<&str>) -> LanguageConfig {
    match mode {
        Some("de") | Some("german") => LanguageConfig::german_primary(),
        Some("en") | Some("english") => LanguageConfig::english_primary(),
        Some("translate") => LanguageConfig::translate_to_english(),
        _ => LanguageConfig::german_english_mixed(), // Default: auto-detect mixed
    }
}

#[derive(Debug)]
struct ResolvedUser {
    user_id: u64,
    display_name: String,
    audio: PreparedAudio,
}

#[derive(Debug)]
struct UserChunks {
    user_id: u64,
    display_name: String,
    chunks: Vec<AudioChunk>,
    total_duration_secs: f32,
}

async fn resolve_user_names(
    db: &db::DbPool,
    guild_id: &str,
    prepared_audio: Vec<PreparedAudio>,
) -> Vec<ResolvedUser> {
    let mut resolved = Vec::new();

    for audio in prepared_audio {
        let user_id_str = audio.user_id.to_string();

        let display_name = match db::get_user_setting(db, &user_id_str, guild_id).await {
            Ok(Some(setting)) if setting.transcribe_name.is_some() => {
                setting.transcribe_name.unwrap()
            }
            _ => {
                format!("User_{}", audio.user_id)
            }
        };

        info!("Resolved user {} -> '{}'", audio.user_id, display_name);

        resolved.push(ResolvedUser {
            user_id: audio.user_id,
            display_name,
            audio,
        });
    }

    resolved
}

/// Transcribe a recording session using Whisper AI
/// 
/// Prepares audio for all users, splits on silence gaps, and transcribes
/// using a local Whisper model (downloaded from Hugging Face if needed).
/// 
/// Supports mixed German/English speech with auto-detection.
#[poise::command(prefix_command, slash_command, rename = "transcribe-session")]
pub async fn transcribe_session(
    ctx: Context<'_>,
    #[description = "Session directory path (e.g. recordings/715908438760357910/2026_01_03_18_49_53)"]
    session_dir: String,
    #[description = "Whisper model size: tiny, base, small, medium, large (default: small)"]
    model: Option<String>,
    #[description = "Language mode: auto (mixed de/en), de (German), en (English), translate (to English)"]
    language: Option<String>,
    #[description = "Minimum silence duration to split chunks (default: 2.0 seconds)"]
    min_silence_secs: Option<f32>,
) -> Result<(), Error> {
    ctx.defer().await?;

    let min_silence = min_silence_secs.unwrap_or(MIN_SILENCE_DURATION_SECS);
    
    // Parse model selection
    let whisper_model = match model.as_deref() {
        Some(m) => m.parse::<WhisperModel>().map_err(|e| -> Error { e.into() })?,
        None => WhisperModel::Small,
    };
    
    // Parse language mode (default: auto-detect mixed German/English)
    let language_config = parse_language_mode(language.as_deref());

    let session_path = PathBuf::from(&session_dir);
    if !session_path.exists() {
        ctx.say(format!("Session directory not found: {}", session_dir))
            .await?;
        return Ok(());
    }

    // Extract guild ID from path (recordings/GUILD_ID/TIMESTAMP)
    let guild_id = session_path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("0")
        .to_string();

    info!("Transcribing session: {} (guild: {}, model: {})", session_dir, guild_id, whisper_model);

    // Determine language mode description
    let lang_desc = match language.as_deref() {
        Some("de") | Some("german") => "German (primary)",
        Some("en") | Some("english") => "English (primary)",
        Some("translate") => "Translate to English",
        _ => "Auto-detect (German/English mixed)",
    };

    // Send initial status
    ctx.say(format!(
        "🎙️ **Starting transcription...**\n\
        Model: `{}` (~{}MB)\n\
        Language: `{}`\n\
        Silence threshold: `{:.1}s`\n\n\
        _This may take a while for long recordings..._",
        whisper_model,
        whisper_model.size_mb(),
        lang_desc,
        min_silence
    )).await?;

    // Prepare audio for all users
    let prepared = match prepare_session_for_transcription(&session_path) {
        Ok(p) => p,
        Err(e) => {
            ctx.say(format!("❌ Failed to prepare session: {}", e)).await?;
            return Ok(());
        }
    };

    info!("Prepared {} users for transcription", prepared.len());

    // Resolve user names from database
    let resolved = resolve_user_names(&ctx.data().db, &guild_id, prepared).await;

    // Create output directory
    let output_dir = session_path.join("transcribe");
    fs::create_dir_all(&output_dir)?;

    // Initialize Whisper (downloads model if needed)
    ctx.channel_id()
        .say(&ctx.http(), format!("⏳ Loading Whisper {} model...", whisper_model))
        .await?;

    let transcriber = match Transcriber::with_language(whisper_model, language_config) {
        Ok(t) => t,
        Err(e) => {
            ctx.say(format!("❌ Failed to initialize Whisper: {}", e)).await?;
            return Ok(());
        }
    };

    // Process each user
    let mut all_transcriptions: Vec<UserTranscription> = Vec::new();
    let mut user_info = Vec::new();

    for user in &resolved {
        let safe_name = user
            .display_name
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
            .collect::<String>();

        // Create user directory
        let user_dir = output_dir.join(format!("{}_{}", user.user_id, safe_name));
        fs::create_dir_all(&user_dir)?;

        // Split audio on silence
        let chunks = user.audio.split_on_silence(min_silence);

        if chunks.is_empty() {
            info!("No audio chunks for user {} (all silence?)", user.display_name);
            continue;
        }

        ctx.channel_id()
            .say(&ctx.http(), format!(
                "🔄 Transcribing **{}**: {} chunks ({:.1}s)...",
                user.display_name,
                chunks.len(),
                user.audio.duration_secs
            ))
            .await?;

        // Write WAV chunks
        for chunk in &chunks {
            let chunk_filename = format!("chunk_{:04}.wav", chunk.index);
            let chunk_path = user_dir.join(&chunk_filename);
            fs::write(&chunk_path, chunk.as_wav_bytes())?;
        }

        // Transcribe all chunks
        let chunk_transcriptions = match transcriber.transcribe_chunks(&chunks) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("Failed to transcribe {}: {}", user.display_name, e);
                user_info.push(format!("• **{}**: ❌ transcription failed", user.display_name));
                continue;
            }
        };

        // Create user transcription with absolute timestamps
        let user_transcription = UserTranscription::from_chunks(
            user.user_id,
            user.display_name.clone(),
            &whisper_model.to_string(),
            user.audio.duration_secs,
            chunk_transcriptions,
        );

        // Write transcription JSON
        let transcription_path = user_dir.join("transcription.json");
        fs::write(
            &transcription_path,
            serde_json::to_string_pretty(&user_transcription)?,
        )?;

        // Write plain text transcript
        let transcript_path = user_dir.join("transcript.txt");
        fs::write(&transcript_path, &user_transcription.full_transcript)?;

        // Write SRT subtitles
        let srt_path = user_dir.join("transcript.srt");
        let srt_content = generate_srt(&user_transcription);
        fs::write(&srt_path, &srt_content)?;

        // Write timing metadata
        let timing_data = serde_json::json!({
            "user_id": user.user_id,
            "display_name": user.display_name,
            "total_duration_secs": user.audio.duration_secs,
            "first_tick": user.audio.first_tick,
            "last_tick": user.audio.last_tick,
            "ssrcs": user.audio.ssrcs,
            "min_silence_secs": min_silence,
            "model": whisper_model.to_string(),
            "chunks": chunks.iter().map(|c| {
                serde_json::json!({
                    "index": c.index,
                    "file": format!("chunk_{:04}.wav", c.index),
                    "start_time_secs": c.start_time_secs,
                    "end_time_secs": c.end_time_secs,
                    "duration_secs": c.duration_secs,
                })
            }).collect::<Vec<_>>()
        });

        let timing_path = user_dir.join("timing.json");
        fs::write(&timing_path, serde_json::to_string_pretty(&timing_data)?)?;

        let word_count = user_transcription.full_transcript.split_whitespace().count();
        user_info.push(format!(
            "• **{}**: {} chunks, ~{} words",
            user.display_name,
            user_transcription.chunk_transcriptions.len(),
            word_count
        ));

        all_transcriptions.push(user_transcription);
    }

    // Write session manifest
    let manifest = serde_json::json!({
        "session": session_dir,
        "guild_id": guild_id,
        "model": whisper_model.to_string(),
        "min_silence_secs": min_silence,
        "users": all_transcriptions.iter().map(|u| {
            serde_json::json!({
                "user_id": u.user_id,
                "display_name": u.display_name,
                "chunk_count": u.chunk_transcriptions.len(),
                "total_duration_secs": u.total_duration_secs,
                "word_count": u.full_transcript.split_whitespace().count(),
                "directory": format!("{}_{}", u.user_id, u.display_name
                    .chars()
                    .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
                    .collect::<String>()),
            })
        }).collect::<Vec<_>>()
    });

    let manifest_path = output_dir.join("manifest.json");
    fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)?;

    // Build final response
    let total_words: usize = all_transcriptions
        .iter()
        .map(|t| t.full_transcript.split_whitespace().count())
        .sum();

    let response = format!(
        "✅ **Transcription complete!**\n\n\
        {}\n\n\
        **Model:** `{}`\n\
        **Total:** ~{} words from {} user(s)\n\
        **Output:** `{}`\n\n\
        _Each user folder contains:_\n\
        • `transcript.txt` - Plain text\n\
        • `transcript.srt` - Subtitles with timestamps\n\
        • `transcription.json` - Full data with timing",
        user_info.join("\n"),
        whisper_model,
        total_words,
        all_transcriptions.len(),
        output_dir.display()
    );

    ctx.say(response).await?;
    Ok(())
}

/// Generate SRT subtitle format from transcription
fn generate_srt(transcription: &UserTranscription) -> String {
    let mut srt = String::new();
    
    for (i, segment) in transcription.all_segments.iter().enumerate() {
        let start = format_srt_time(segment.start_secs);
        let end = format_srt_time(segment.end_secs);
        
        srt.push_str(&format!(
            "{}\n{} --> {}\n{}\n\n",
            i + 1,
            start,
            end,
            segment.text
        ));
    }
    
    srt
}

/// Format seconds as SRT timestamp (HH:MM:SS,mmm)
fn format_srt_time(secs: f32) -> String {
    let hours = (secs / 3600.0) as u32;
    let minutes = ((secs % 3600.0) / 60.0) as u32;
    let seconds = (secs % 60.0) as u32;
    let millis = ((secs % 1.0) * 1000.0) as u32;
    
    format!("{:02}:{:02}:{:02},{:03}", hours, minutes, seconds, millis)
}
