use crate::{Context, Error};
use crate::db;

#[poise::command(prefix_command, slash_command)]
pub async fn set_transcribe_name(
    ctx: Context<'_>,
    #[description = "The new name for the transcribe"] new_name: String,
) -> Result<(), Error> {
    let user_id = &ctx.author().id.to_string();
    let guild_id = &ctx.guild_id().unwrap().to_string();

    db::set_transcribe_name(&ctx.data().db, user_id, guild_id, &new_name).await?;

    ctx.say(format!("Successfully set the name of the transcribe to {new_name}!")).await?;
    Ok(())
}