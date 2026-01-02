use crate::{Context, Error};

#[poise::command(prefix_command, slash_command)]
pub async fn set_descriptor_name(
    ctx: Context<'_>,
    #[description = "The new name for the descriptor"] new_name: String,
) -> Result<(), Error> {

    // get user and guild id 
    let user_id = ctx.author().id;
    let guild_id = ctx.guild_id();
    let response = format!("Successfully set the name of the descriptor to {new_name}!");
    ctx.say(response).await?;
    Ok(())
}