use crate::Context;
use crate::Error;
use tracing::{error, info};

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

#[poise::command(prefix_command, slash_command, rename = "stop-recording", guild_only)]
pub async fn stop_recording(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = ctx
        .guild_id()
        .ok_or("This command must be used in a guild")?;
    let guild_id_u64 = guild_id.get();

    let session = {
        let mut sessions = ctx.data().active_sessions.lock().await;
        sessions.remove(&guild_id_u64)
    };

    let mut session = match session {
        Some(s) => s,
        None => {
            ctx.say("No recording is active on this guild.").await?;
            return Ok(());
        }
    };

    ctx.defer().await?;

    let storage_handle = {
        let mut state = session.state.lock().await;
        state.stop()
    };

    if let Some(handle) = storage_handle {
        handle.shutdown();
    }

    if let Some(task) = session.storage_task.take() {
        if let Err(e) = task.await {
            error!("Storage task panicked: {:?}", e);
        }
    }

    let manager = songbird::get(ctx.serenity_context())
        .await
        .ok_or("Songbird voice client not initialized")?
        .clone();

    if let Err(e) = manager.remove(guild_id).await {
        error!("Failed to leave voice channel: {:?}", e);
    }

    info!("Left voice channel in guild {}", guild_id);

    let duration = session.duration();
    let duration_str = format_duration(duration);

    ctx.say(format!(
        "🎙️ **Recording stopped!**\n\
        📁 Session: `{}`\n\
        ⏱️ Duration: {}",
        session.session_dir.display(),
        duration_str
    ))
    .await?;
    Ok(())
}
