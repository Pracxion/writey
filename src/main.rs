use anyhow::Context as _;
use serenity::{
    async_trait,
    model::{
        channel::Message,
        gateway::Ready,
    },
    prelude::*,
};
use tracing::info;
use dotenvy::dotenv;

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _ctx: Context, ready: Ready) {
        info!("Logged in as {}", ready.user.name);
        info!("Bot ID: {}", ready.user.id);
        info!("Connected to {} guilds", ready.guilds.len());
        info!("Gateway version: {:?}", ready.version);
    }

    async fn message(&self, ctx: Context, msg: Message) {
        if msg.author.bot {
            return;
        }

        let content = msg.content.trim();
        info!("Message: {}", content);
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv().ok();
    tracing_subscriber::fmt::init();

    let token = std::env::var("DISCORD_TOKEN")
        .context("Set DISCORD_TOKEN environment variable")?;

    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT
        | GatewayIntents::GUILD_VOICE_STATES;

    let mut client = Client::builder(token, intents)
        .event_handler(Handler).await?;

    client.start().await?;
    Ok(())
}
