use crate::db;
use crate::Context;
use crate::Error;

#[poise::command(prefix_command, slash_command, rename = "set-transcribe-name")]
pub async fn set_transcribe_name(
    ctx: Context<'_>,
    #[description = "The new name for the transcribe"] new_name: String,
) -> Result<(), Error> {
    let user_id = &ctx.author().id.to_string();
    let guild_id = &ctx.guild_id().unwrap().to_string();

    db::set_transcribe_name(&ctx.data().db, user_id, guild_id, &new_name).await?;

    ctx.say(format!("Set Transcribtion Name to {new_name}!"))
        .await?;
    Ok(())
}

