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
use songbird::{Config, SerenityInit, Songbird, driver::DecodeMode};
use std::{
    collections::HashMap,
    sync::Arc,
    time::Duration,
    path::PathBuf,
};
use tokio::sync::Mutex;
use tracing::info;
use dotenvy::dotenv;

mod command;
mod db;
mod voice;
mod transcription;

use command::command::*;
use db::DbPool;
use voice::{SharedRecordingState, SessionStorage};

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Context<'a> = poise::Context<'a, Data, Error>;

pub struct RecordingSession {
    pub started_by: u64,
    pub guild_id: u64,
    pub session_id: String,
    pub session_dir: PathBuf,
    pub state: SharedRecordingState,
    pub started_at: chrono::DateTime<chrono::Utc>,
}

impl RecordingSession {
    pub fn new(guild_id: u64, started_by: u64) -> Self {
        let timestamp = chrono::Utc::now();
        let session_id = format!(
            "{}_{}", 
            guild_id,
            timestamp.format("%Y%m%d_%H%M%S")
        );
        let session_dir = PathBuf::from("recordings").join(&session_id);

        Self {
            started_by,
            guild_id,
            session_id,
            session_dir,
            state: voice::create_recording_session(),
            started_at: timestamp,
        }
    }

    pub fn duration(&self) -> chrono::Duration {
        chrono::Utc::now() - self.started_at
    }
}

pub struct Data {
    pub active_sessions: Mutex<HashMap<u64, RecordingSession>>,
    pub db: DbPool,
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

    let database_url = std::env::var("DATABASE_URL").unwrap();
    let db_pool = db::init_db(&database_url)
        .await
        .context("Failed to initialize database")?;
    info!("Database initialized successfully");

    std::fs::create_dir_all("recordings").ok();
    std::fs::create_dir_all("exports").ok();
    std::fs::create_dir_all("models").ok();

    let options = poise::FrameworkOptions {
        commands: vec![
            set_transcribe_name(),
            get_transcribe_name(),
            list_voice_users(),
            start_recording(),
            stop_recording(),
        ],
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
                println!("Got an event in event handler: {:?}", event.snake_case_name());
                Ok(())
            })
        },
        ..Default::default()
    };
    
    let token = std::env::var("DISCORD_TOKEN")
        .context("Set DISCORD_TOKEN environment variable")?;

    let intents = GatewayIntents::GUILDS
        | GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT
        | GatewayIntents::GUILD_VOICE_STATES;

    let framework = poise::Framework::builder()
        .setup(move |ctx, _ready, framework| {
            let db = db_pool.clone();
            Box::pin(async move {
                println!("Logged in as {}", _ready.user.name);
                poise::builtins::register_globally(ctx, &framework.options().commands).await?;
                
                if let Ok(guild_id_str) = std::env::var("GUILD_ID") {
                    if let Ok(guild_id) = guild_id_str.parse::<u64>() {
                        let guild_id = serenity::model::id::GuildId::new(guild_id);
                        poise::builtins::register_in_guild(ctx, &framework.options().commands, guild_id).await?;
                        info!("Registered commands for guild {}", guild_id);
                    } else {
                        eprintln!("Invalid GUILD_ID format: {}", guild_id_str);
                    }
                }
                
                Ok(Data {
                    active_sessions: Mutex::new(HashMap::new()),
                    db,
                })
            })
        })
        .options(options)
        .build();

    let songbird_config = Config::default().decode_mode(DecodeMode::Decode);
    
    let mut client = Client::builder(token, intents)
        .framework(framework)
        .register_songbird_from_config(songbird_config)
        .await?;

    client.start().await?;
    Ok(())
}
