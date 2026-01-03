use crate::Context;
use crate::Error;
use poise::serenity_prelude as serenity;

#[poise::command(prefix_command, slash_command, rename = "list-voice-users", guild_only)]
pub async fn list_voice_users(
    ctx: Context<'_>,
    #[description = "Voice channel to list users from (leave empty for your current channel)"]
    channel: Option<serenity::model::channel::Channel>,
) -> Result<(), Error> {
    let guild_id = ctx
        .guild_id()
        .ok_or("This command must be used in a guild")?;
    let user_id = ctx.author().id;

    let voice_channel_id = if let Some(ch) = channel {
        match ch {
            serenity::model::channel::Channel::Guild(ch) => {
                if ch.kind == serenity::model::channel::ChannelType::Voice {
                    ch.id
                } else {
                    ctx.say("The specified channel is not a voice channel!")
                        .await?;
                    return Ok(());
                }
            }
            _ => {
                ctx.say("Invalid channel type!").await?;
                return Ok(());
            }
        }
    } else {
        let cache = &ctx.serenity_context().cache;

        let channel_id = cache.guild(guild_id).and_then(|guild| {
            guild
                .voice_states
                .get(&user_id)
                .and_then(|vs| vs.channel_id)
        });

        match channel_id {
            Some(id) => id,
            None => {
                ctx.say("You're not in a voice channel. Please join one or specify a channel: `/list-voice-users channel:#voice-channel`").await?;
                return Ok(());
            }
        }
    };

    let cache = &ctx.serenity_context().cache;
    let http = ctx.serenity_context().http.clone();

    let user_ids_in_channel: Vec<u64> = {
        let guild = cache.guild(guild_id).ok_or("Guild not found in cache")?;
        guild
            .voice_states
            .iter()
            .filter(|(_, vs)| vs.channel_id == Some(voice_channel_id))
            .map(|(uid, _)| uid.get())
            .collect()
    };

    let mut users_in_channel = Vec::new();
    for user_id in user_ids_in_channel {
        let user_id_serenity = serenity::model::id::UserId::new(user_id);

        if let Some(user) = cache.user(user_id_serenity) {
            let display_name = user
                .global_name
                .as_deref()
                .unwrap_or_else(|| user.name.as_str());

            users_in_channel.push((user_id, display_name.to_string(), user.name.clone()));
        } else {
            if let Ok(user) = http.get_user(user_id_serenity).await {
                let display_name = user
                    .global_name
                    .as_deref()
                    .unwrap_or_else(|| user.name.as_str());
                users_in_channel.push((user_id, display_name.to_string(), user.name.clone()));
            } else {
                users_in_channel.push((
                    user_id,
                    format!("User {}", user_id),
                    format!("User {}", user_id),
                ));
            }
        }
    }

    if users_in_channel.is_empty() {
        ctx.say(format!(
            "No users found in voice channel <#{}>.",
            voice_channel_id
        ))
        .await?;
        return Ok(());
    }

    let mut response = format!("**Users in <#{}>:**\n", voice_channel_id);
    for (user_id, display_name, username) in users_in_channel {
        response.push_str(&format!(
            "- **{}** (`{}`) - ID: `{}`\n",
            display_name, username, user_id
        ));
    }

    ctx.say(response).await?;
    Ok(())
}
