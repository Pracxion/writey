use crate::voice::receiver::{mix_audio_buffers, save_to_wav, Receiver, CHANNELS, SAMPLE_RATE};
use crate::{Context, Error, RecordingSession};
use crate::db;
use poise::serenity_prelude as serenity;
use songbird::CoreEvent;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
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
    let guild_id_str = guild_id.to_string();
    let user_id = ctx.author().id;
    let user_id_str = user_id.to_string();

    // Check if there's already an active recording session
    {
        let sessions = ctx.data().active_sessions.lock().await;
        if sessions.contains_key(&guild_id_str) {
            ctx.say("A recording is already active on this server.").await?;
            return Ok(());
        }
    }

    // Get the user's current voice channel
    let voice_channel_id = {
        let guild = ctx
            .guild()
            .ok_or("Could not find guild")?;
        
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

    // Get the Songbird manager
    let manager = songbird::get(ctx.serenity_context())
        .await
        .ok_or("Songbird voice client not initialized")?
        .clone();

    // Join the voice channel
    let handler_lock = match manager.join(guild_id, voice_channel_id).await {
        Ok(handler) => handler,
        Err(e) => {
            error!("Failed to join voice channel: {:?}", e);
            ctx.say(format!("Failed to join voice channel: {:?}", e)).await?;
            return Ok(());
        }
    };

    info!("Joined voice channel {} in guild {}", voice_channel_id, guild_id);

    // Create the recording session data
    let ssrc_map = Arc::new(Mutex::new(HashMap::new()));
    let audio_buffers = Arc::new(Mutex::new(HashMap::new()));
    let recording_active = Arc::new(Mutex::new(true));

    // Create the receiver and register events
    let receiver = Receiver::new(
        Arc::clone(&ssrc_map),
        Arc::clone(&audio_buffers),
        Arc::clone(&recording_active),
    );

    {
        let mut handler = handler_lock.lock().await;
        
        // Subscribe to speaking state updates to map SSRC -> User ID
        handler.add_global_event(CoreEvent::SpeakingStateUpdate.into(), receiver);
        
        // Create a new receiver for VoiceTick events
        let voice_tick_receiver = Receiver::new(
            Arc::clone(&ssrc_map),
            Arc::clone(&audio_buffers),
            Arc::clone(&recording_active),
        );
        
        // Subscribe to voice tick events for recording
        handler.add_global_event(CoreEvent::VoiceTick.into(), voice_tick_receiver);
    }

    // Store the recording session
    {
        let mut sessions = ctx.data().active_sessions.lock().await;
        sessions.insert(
            guild_id_str.clone(),
            RecordingSession {
                started_by: user_id_str,
                ssrc_map,
                audio_buffers,
                recording_active,
            },
        );
    }

    ctx.say("🎙️ Started recording! You can stop the recording with `/stop-recording`.").await?;
    Ok(())
}

#[poise::command(prefix_command, slash_command, rename = "stop-recording", guild_only)]
pub async fn stop_recording(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = ctx.guild_id().ok_or("This command must be used in a guild")?;
    let guild_id_str = guild_id.to_string();

    // Get and remove the recording session
    let session = {
        let mut sessions = ctx.data().active_sessions.lock().await;
        sessions.remove(&guild_id_str)
    };

    let session = match session {
        Some(s) => s,
        None => {
            ctx.say("No recording is active on this server.").await?;
            return Ok(());
        }
    };

    // Signal to stop recording
    {
        let mut recording_active = session.recording_active.lock().await;
        *recording_active = false;
    }

    // Get the Songbird manager and leave the voice channel
    let manager = songbird::get(ctx.serenity_context())
        .await
        .ok_or("Songbird voice client not initialized")?
        .clone();

    if let Err(e) = manager.remove(guild_id).await {
        error!("Failed to leave voice channel: {:?}", e);
    }

    info!("Left voice channel in guild {}", guild_id);

    // Get the audio buffers and mix them
    let audio_buffers = session.audio_buffers.lock().await;
    
    if audio_buffers.is_empty() {
        ctx.say("⚠️ Recording stopped, but no audio was captured.").await?;
        return Ok(());
    }

    // Mix all user audio into a single buffer
    let mixed_audio = mix_audio_buffers(&audio_buffers);

    if mixed_audio.is_empty() {
        ctx.say("⚠️ Recording stopped, but no audio data was captured.").await?;
        return Ok(());
    }

    // Generate filename with timestamp
    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let filename = format!("recordings/recording_{}_{}.wav", guild_id_str, timestamp);

    // Ensure recordings directory exists
    if let Err(e) = std::fs::create_dir_all("recordings") {
        error!("Failed to create recordings directory: {:?}", e);
        ctx.say(format!("Failed to create recordings directory: {:?}", e)).await?;
        return Ok(());
    }

    // Save to WAV file
    match save_to_wav(&mixed_audio, &filename, SAMPLE_RATE, CHANNELS) {
        Ok(_) => {
            let duration_secs = mixed_audio.len() as f64 / (SAMPLE_RATE as f64 * CHANNELS as f64);
            ctx.say(format!(
                "🎙️ Recording stopped and saved!\n📁 File: `{}`\n⏱️ Duration: {:.1} seconds\n👥 Users recorded: {}",
                filename,
                duration_secs,
                audio_buffers.len()
            )).await?;
        }
        Err(e) => {
            error!("Failed to save WAV file: {:?}", e);
            ctx.say(format!("Failed to save recording: {:?}", e)).await?;
        }
    }

    Ok(())
}
