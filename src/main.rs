use anyhow::Context as _;
use poise::serenity_prelude as serenity;
use serenity::{
    async_trait,
    model::{
        channel::Message,
        gateway::Ready,
        gateway::GatewayIntents,
    },
    prelude::*,
    Client,
};
use std::{
    collections::HashMap,
    sync::Arc,
    time::Duration,
};
use std::sync::Mutex;
use tracing::info;
use dotenvy::dotenv;

mod command;
use command::command::*;

// Types used by all command functions
pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Context<'a> = poise::Context<'a, Data, Error>;

// Custom user data passed to all command functions
pub struct Data {
    pub votes: Mutex<HashMap<String, u64>>,
}

async fn on_error(error: poise::FrameworkError<'_, Data, Error>) {
    match error {
        poise::FrameworkError::Setup { error, .. } => panic!("Failed to start bot: {:?}", error),
        poise::FrameworkError::Command { error, ctx, .. } => {
            println!("Error in command `{}`: {:?}", ctx.command().name, error,);
        }
        error => {
            if let Err(e) = poise::builtins::on_error(error).await {
                println!("Error while handling error: {}", e)
            }
        }
    }
}

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _ctx: serenity::Context, ready: Ready) {
        info!("Logged in as {}", ready.user.name);
        info!("Bot ID: {}", ready.user.id);
        info!("Connected to {} guilds", ready.guilds.len());
        info!("Gateway version: {:?}", ready.version);
    }

    async fn message(&self, ctx: serenity::Context, msg: Message) {
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

    // Every option can be omitted to use its default value
    let options = poise::FrameworkOptions {
        commands: vec![set_descriptor_name()],
        prefix_options: poise::PrefixFrameworkOptions {
            prefix: Some("/".into()),
            edit_tracker: Some(Arc::new(poise::EditTracker::for_timespan(
                Duration::from_secs(3600),
            ))),
            ..Default::default()
        },
        // The global error handler for all error cases that may occur
        on_error: |error| Box::pin(on_error(error)),
        // This code is run before every command
        pre_command: |ctx| {
            Box::pin(async move {
                println!("Executing command {}...", ctx.command().qualified_name);
            })
        },
        // This code is run after a command if it was successful (returned Ok)
        post_command: |ctx| {
            Box::pin(async move {
                println!("Executed command {}!", ctx.command().qualified_name);
            })
        },
        // Every command invocation must pass this check to continue execution
        command_check: Some(|ctx| {
            Box::pin(async move {
                if ctx.author().id.get() == 123456789 {
                    return Ok(false);
                }
                Ok(true)
            })
        }),
        // Enforce command checks even for owners (enforced by default)
        // Set to true to bypass checks, which is useful for testing
        skip_checks_for_owners: false,
        event_handler: |_ctx, event, _framework, _data| {
            Box::pin(async move {
                println!("Got an event in event handler: {:?}", event);
                Ok(())
            })
        },
        ..Default::default()
    };
    
    let token = std::env::var("DISCORD_TOKEN")
        .context("Set DISCORD_TOKEN environment variable")?;

    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT
        | GatewayIntents::GUILD_VOICE_STATES;

    let framework = poise::Framework::builder()
        .setup(move |ctx, _ready, framework| {
            Box::pin(async move {
                println!("Logged in as {}", _ready.user.name);
                poise::builtins::register_globally(ctx, &framework.options().commands).await?;
                Ok(Data {
                    votes: Mutex::new(HashMap::new()),
                })
            })
        })
        .options(options)
        .build();

    let mut client = Client::builder(token, intents)
        .framework(framework)
        .await?;

    client.start().await?;
    Ok(())
}
