use crate::voice::{Receiver, StorageWriter};
use crate::Context;
use crate::Error;
use crate::RecordingSession;
use poise::serenity_prelude as serenity;
use songbird::CoreEvent;
use std::sync::Arc;
use tracing::{error, info};

async fn get_voice_channel(
    ctx: Context<'_>,
    guild_id: serenity::model::id::GuildId,
    user_id: serenity::model::id::UserId,
    channel: Option<serenity::model::channel::Channel>,
) -> Result<Option<serenity::model::id::ChannelId>, Error> {
    match channel {
        Some(ch) => {
            match ch {
                serenity::model::channel::Channel::Guild(ch) => {
                    if ch.kind == serenity::model::channel::ChannelType::Voice {
                        Ok(Some(ch.id))
                    } else {
                        ctx.say("The specified channel is not a voice channel!")
                            .await?;
                        Ok(None)
                    }
                }
                _ => {
                    ctx.say("Invalid channel type!").await?;
                    Ok(None)
                }
            }
        }
        None => {
            let cache = &ctx.serenity_context().cache;
            let channel_id = cache.guild(guild_id).and_then(|guild| {
                guild
                    .voice_states
                    .get(&user_id)
                    .and_then(|vs| vs.channel_id)
            });
            match channel_id {
                Some(id) => Ok(Some(id)),
                None => {
                    ctx.say("You're not in a voice channel. Please join one or specify a channel: `/start-recording channel:#your-voice-channel`").await?;
                    Ok(None)
                }
            }
        }
    }
}

#[poise::command(prefix_command, slash_command, rename = "start-recording", guild_only)]
pub async fn start_recording(
    ctx: Context<'_>,
    #[description = "Voice channel to record (leave empty to auto-detect)"] channel: Option<
        serenity::model::channel::Channel,
    >,
) -> Result<(), Error> {
    let guild_id = ctx
        .guild_id()
        .ok_or("This command must be used in a guild")?;
    let guild_id_u64 = guild_id.get();
    let user_id = ctx.author().id;
    let user_id_u64 = user_id.get();

    {
        let sessions = ctx.data().active_sessions.lock().await;
        if sessions.contains_key(&guild_id_u64) {
            ctx.say("A recording is already active on this guild.")
                .await?;
            return Ok(());
        }
    }

    let voice_channel_id = match get_voice_channel(ctx, guild_id, user_id, channel).await? {
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

    let mut session = RecordingSession::new(guild_id_u64, user_id_u64);

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

    let session_id = session.session_id.clone();

    {
        let mut sessions = ctx.data().active_sessions.lock().await;
        sessions.insert(guild_id_u64, session);
    }

    ctx.say(format!(
        "🎙️ **Recording started!**\n\
        📁 Session: `{}`",
        session_id
    ))
    .await?;

    Ok(())
}

