use crate::Context;
use crate::Error;
use crate::RecordingSession;
use crate::voice::{Receiver, StorageWriter, resolve_voice_channel};
use poise::serenity_prelude::model::channel::Channel;
use songbird::CoreEvent;
use std::sync::Arc;
use tracing::{error, info};

#[poise::command(prefix_command, slash_command, rename = "start-recording", guild_only)]
pub async fn start_recording(
    ctx: Context<'_>,
    #[description = "Voice channel to record (leave empty to auto-detect)"] channel: Option<
        Channel,
    >,
) -> Result<(), Error> {
    let guild_id = ctx
        .guild_id()
        .ok_or("This command must be used in a guild")?;
    let guild_id_u64 = guild_id.get();
    let user_id = ctx.author().id;

    {
        let sessions = ctx.data().active_sessions.lock().await;
        if sessions.contains_key(&guild_id_u64) {
            ctx.say("A recording is already active on this guild.")
                .await?;
            return Ok(());
        }
    }

    let voice_channel_id = match resolve_voice_channel(ctx, guild_id, user_id, channel).await? {
        Some(id) => id,
        None => return Ok(()),
    };

    let manager = songbird::get(ctx.serenity_context())
        .await
        .ok_or("Songbird voice client not initialized")?
        .clone();

    let handler_lock = match manager.join(guild_id, voice_channel_id).await {
        Ok(handler) => handler,
        Err(e) => {
            error!("Failed to join voice channel: {:?}", e);
            ctx.say(format!("Failed to join voice channel: {:?}", e))
                .await?;
            return Ok(());
        }
    };

    info!(
        "Joined voice channel {} in guild {}",
        voice_channel_id, guild_id
    );

    let mut session = RecordingSession::new(guild_id_u64);

    let (storage_handle, storage_writer) = match StorageWriter::new(session.session_dir.clone()) {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to create session storage: {:?}", e);
            let _ = manager.remove(guild_id).await;
            ctx.say(format!("Failed to create storage: {:?}", e))
                .await?;
            return Ok(());
        }
    };

    let storage_task = tokio::spawn(async move {
        storage_writer.run().await;
    });
    session.storage_task = Some(storage_task);

    {
        let mut state = session.state.lock().await;
        state.start(storage_handle);
    }

    let receiver = Receiver::new(Arc::clone(&session.state));

    {
        let mut handler = handler_lock.lock().await;

        handler.add_global_event(CoreEvent::SpeakingStateUpdate.into(), receiver);

        let voice_tick_receiver = Receiver::new(Arc::clone(&session.state));

        handler.add_global_event(CoreEvent::VoiceTick.into(), voice_tick_receiver);
    }

    let session_dir_display = session.session_dir.display().to_string();

    {
        let mut sessions = ctx.data().active_sessions.lock().await;
        sessions.insert(guild_id_u64, session);
    }

    ctx.say(format!(
        "🎙️ **Recording started!**\n\
        📁 Session: `{}`",
        session_dir_display
    ))
    .await?;

    Ok(())
}
