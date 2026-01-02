use crate::{Context, Error};
use crate::db;

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