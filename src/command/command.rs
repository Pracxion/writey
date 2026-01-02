use crate::voice::{
    Receiver, SessionStorage, SessionExporter, ExportConfig,
    SAMPLE_RATE, CHANNELS,
};
use crate::{Context, Error, RecordingSession};
use crate::db;
use crate::transcription::{Transcript, TranscriptSegment, ExportFormat};
use poise::serenity_prelude as serenity;
use songbird::CoreEvent;
use std::sync::Arc;
use tracing::{error, info};

#[poise::command(prefix_command, slash_command, rename = "set-transcribe-name")]
pub async fn set_transcribe_name(
    ctx: Context<'_>,
    #[description = "The new name for the transcribe"] new_name: String,
) -> Result<(), Error> {
    let user_id = &ctx.author().id.to_string();
    let guild_id = &ctx.guild_id().unwrap().to_string();

    db::set_transcribe_name(&ctx.data().db, user_id, guild_id, &new_name).await?;

    ctx.say(format!("Set Transcribtion Name to {new_name}!")).await?;
    Ok(())
}

#[poise::command(prefix_command, slash_command, rename = "get-transcribe-name")]
pub async fn get_transcribe_name(ctx: Context<'_>) -> Result<(), Error> {
    let user_id = &ctx.author().id.to_string();
    let guild_id = &ctx.guild_id().unwrap().to_string();

    let user_setting = db::get_user_setting(&ctx.data().db, user_id, guild_id).await?;

    if user_setting.is_none() {
        ctx.say("No Transcribtion Name set on this server.").await?;
        return Ok(());
    }

    let transcribe_name = user_setting.unwrap().transcribe_name;
    match transcribe_name {
        Some(name) => {
            ctx.say(format!("Transcribtion Name is {name}!")).await?;
        }
        None => {
            ctx.say("No Transcribtion Name set on this server.").await?;
        }
    }

    Ok(())
}

#[poise::command(prefix_command, slash_command, rename = "start-recording", guild_only)]
pub async fn start_recording(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = ctx.guild_id().ok_or("This command must be used in a guild")?;
    let guild_id_u64 = guild_id.get();
    let user_id = ctx.author().id;
    let user_id_u64 = user_id.get();

    {
        let sessions = ctx.data().active_sessions.lock().await;
        if sessions.contains_key(&guild_id_u64) {
            ctx.say("A recording is already active on this guild.").await?;
            return Ok(());
        }
    }

    let voice_channel_id = {
        let guild = ctx
            .guild()
            .ok_or("Could not find guild id")?;
        
        guild
            .voice_states
            .get(&user_id)
            .and_then(|vs| vs.channel_id)
    };

    let voice_channel_id = match voice_channel_id {
        Some(id) => id,
        None => {
            ctx.say("You must be in a voice channel to start recording!").await?;
            return Ok(());
        }
    };

    let manager = songbird::get(ctx.serenity_context())
        .await
        .ok_or("Songbird voice client not initialized")?
        .clone();

    let handler_lock = match manager.join(guild_id, voice_channel_id).await {
        Ok(handler) => handler,
        Err(e) => {
            error!("Failed to join voice channel: {:?}", e);
            ctx.say(format!("Failed to join voice channel: {:?}", e)).await?;
            return Ok(());
        }
    };

    info!("Joined voice channel {} in guild {}", voice_channel_id, guild_id);

    let session = RecordingSession::new(guild_id_u64, user_id_u64);
    
    let storage = match SessionStorage::new(
        session.session_dir.clone(),
        SAMPLE_RATE,
        CHANNELS,
    ) {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to create session storage: {:?}", e);
            let _ = manager.remove(guild_id).await;
            ctx.say(format!("Failed to create storage: {:?}", e)).await?;
            return Ok(());
        }
    };

    {
        let mut state = session.state.lock().await;
        state.start(storage);
    }

    let receiver = Receiver::new(Arc::clone(&session.state));

    {
        let mut handler = handler_lock.lock().await;
        
        handler.add_global_event(CoreEvent::SpeakingStateUpdate.into(), receiver);
        
        let voice_tick_receiver = Receiver::new(Arc::clone(&session.state));
        
        handler.add_global_event(CoreEvent::VoiceTick.into(), voice_tick_receiver);
    }

    let session_id = session.session_id.clone();

    {
        let mut sessions = ctx.data().active_sessions.lock().await;
        sessions.insert(guild_id_u64, session);
    }

    ctx.say(format!(
        "🎙️ **Recording started!**\n\
        📁 Session: `{}`\n\
        ⏱️ Use `/stop-recording` to stop and save.",
        session_id
    )).await?;
    
    Ok(())
}

#[poise::command(prefix_command, slash_command, rename = "stop-recording", guild_only)]
pub async fn stop_recording(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = ctx.guild_id().ok_or("This command must be used in a guild")?;
    let guild_id_u64 = guild_id.get();

    let session = {
        let mut sessions = ctx.data().active_sessions.lock().await;
        sessions.remove(&guild_id_u64)
    };

    let session = match session {
        Some(s) => s,
        None => {
            ctx.say("No recording is active on this guild.").await?;
            return Ok(());
        }
    };

    ctx.defer().await?;

    let storage = {
        let mut state = session.state.lock().await;
        state.stop()
    };

    let manager = songbird::get(ctx.serenity_context())
        .await
        .ok_or("Songbird voice client not initialized")?
        .clone();

    if let Err(e) = manager.remove(guild_id).await {
        error!("Failed to leave voice channel: {:?}", e);
    }

    info!("Left voice channel in guild {}", guild_id);

    let user_files = match storage {
        Some(s) => {
            match s.finalize() {
                Ok(files) => files,
                Err(e) => {
                    error!("Failed to finalize storage: {:?}", e);
                    ctx.say(format!("Recording stopped but failed to save: {:?}", e)).await?;
                    return Ok(());
                }
            }
        }
        None => {
            ctx.say("Recording stopped, but no storage was active.").await?;
            return Ok(());
        }
    };

    if user_files.is_empty() {
        ctx.say("Recording stopped, but no audio was captured.").await?;
        return Ok(());
    }

    let duration = session.duration();
    let duration_str = format_duration(duration);

    // Export the session
    let export_config = ExportConfig {
        output_dir: std::path::PathBuf::from("exports"),
        per_user_wav: true,
        mixed_wav: true,
        prepare_for_stt: true,
        transcript_formats: vec![ExportFormat::JsonPretty, ExportFormat::Vtt, ExportFormat::Srt],
    };

    let exporter = SessionExporter::new(export_config);
    
    match exporter.export_session(&session.session_dir, &session.session_id) {
        Ok(result) => {
            let mut response = format!(
                "🎙️ **Recording stopped and saved!**\n\
                📁 Session: `{}`\n\
                ⏱️ Duration: {}\n\
                👥 Users recorded: {}\n",
                session.session_id,
                duration_str,
                result.user_count,
            );

            if let Some(ref path) = result.mixed_wav_path {
                response.push_str(&format!("🔊 Mixed audio: `{}`\n", path.display()));
            }

            if !result.stt_segment_paths.is_empty() {
                response.push_str(&format!(
                    "📝 STT segments: {} files ready for transcription\n",
                    result.stt_segment_paths.len()
                ));
            }

            ctx.say(response).await?;
        }
        Err(e) => {
            error!("Failed to export session: {:?}", e);
            
            // Still report success with basic info
            ctx.say(format!(
                "🎙️ **Recording stopped!**\n\
                📁 Session: `{}`\n\
                ⏱️ Duration: {}\n\
                👥 Users: {}\n\
                ⚠️ Export warning: {:?}",
                session.session_id,
                duration_str,
                user_files.len(),
                e
            )).await?;
        }
    }

    Ok(())
}

/// Format a duration nicely
fn format_duration(duration: chrono::Duration) -> String {
    let total_secs = duration.num_seconds();
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;

    if hours > 0 {
        format!("{}h {}m {}s", hours, minutes, seconds)
    } else if minutes > 0 {
        format!("{}m {}s", minutes, seconds)
    } else {
        format!("{}s", seconds)
    }
}
