use crate::{Context, Error};
use poise::serenity_prelude as serenity;
use serenity::model::{
    channel::{Channel, ChannelType},
    id::{ChannelId, GuildId, UserId},
};

/// Resolve the target voice channel from an optional explicit channel argument.
///
/// If `channel` is `None`, auto-detects the caller's current voice channel.
/// Returns `Ok(None)` after sending an error reply if the channel is invalid.
pub async fn resolve_voice_channel(
    ctx: Context<'_>,
    guild_id: GuildId,
    user_id: UserId,
    channel: Option<Channel>,
) -> Result<Option<ChannelId>, Error> {
    match channel {
        Some(ch) => match ch {
            Channel::Guild(ch) if ch.kind == ChannelType::Voice => Ok(Some(ch.id)),
            Channel::Guild(_) => {
                ctx.say("The specified channel is not a voice channel!")
                    .await?;
                Ok(None)
            }
            _ => {
                ctx.say("Invalid channel type!").await?;
                Ok(None)
            }
        },
        None => {
            let channel_id = ctx
                .serenity_context()
                .cache
                .guild(guild_id)
                .and_then(|guild| {
                    guild
                        .voice_states
                        .get(&user_id)
                        .and_then(|vs| vs.channel_id)
                });

            match channel_id {
                Some(id) => Ok(Some(id)),
                None => {
                    ctx.say(
                        "You're not in a voice channel. \
                         Please join one or specify a channel.",
                    )
                    .await?;
                    Ok(None)
                }
            }
        }
    }
}
